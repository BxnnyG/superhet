import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthError, type AuthClient, type AuthStatus } from "./api";
import { AuthGate } from "./AuthGate";
import { useAuth } from "./context";
import { notifyUnauthorized, requestRecheck, resetSession } from "./session";
import { GUEST_LIMITS, accountStatus, guestStatus, makeStatus, stubClient } from "./testing";

afterEach(() => {
  window.history.replaceState(null, "", "/");
  resetSession();
});

function client(status: AuthStatus | AuthStatus[], over: Partial<AuthClient> = {}): AuthClient {
  const queue = Array.isArray(status) ? [...status] : [status];
  return stubClient({
    status: vi.fn(() =>
      Promise.resolve(queue.length > 1 ? (queue.shift() as AuthStatus) : (queue[0] as AuthStatus)),
    ),
    guestStart: vi.fn().mockResolvedValue(GUEST_LIMITS),
    ...over,
  });
}

function Probe() {
  const auth = useAuth();
  const p = auth?.permissions;
  return (
    <p>
      app {auth?.username} {auth?.role} {p?.canTune ? "tunes" : "listens"}
      {p?.guestLimits ? ` max ${p.guestLimits.maxVfos}` : ""}
    </p>
  );
}

const type = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label, { selector: "input" }), { target: { value } });

const GUEST_ON = makeStatus({
  guest: { enabled: true, privacyText: "Your IP is logged for 7 days." },
});

describe("guest entry", () => {
  it("offers 'Continue as guest' only while guest mode is on", async () => {
    const { unmount } = render(<AuthGate client={client(makeStatus())}>x</AuthGate>);
    await screen.findByRole("heading", { name: "Sign in" });
    expect(screen.queryByRole("button", { name: "Continue as guest" })).toBeNull();
    unmount();
    render(<AuthGate client={client(GUEST_ON)}>x</AuthGate>);
    expect(await screen.findByRole("button", { name: "Continue as guest" })).toBeInTheDocument();
  });

  it("shows the privacy text first, then starts the guest session", async () => {
    const c = client([GUEST_ON, guestStatus()]);
    render(
      <AuthGate client={c}>
        <Probe />
      </AuthGate>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Continue as guest" }));
    expect(await screen.findByText("Your IP is logged for 7 days.")).toBeInTheDocument();
    expect(c.guestStart).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Accept and continue as guest" }));
    // The guest limits (one receiver, WFM/NFM/AM) reach the permissions.
    expect(await screen.findByText(/app Guest listener listens max 1/)).toBeInTheDocument();
  });

  it("explains a full guest list and can go back", async () => {
    const guestStart = vi.fn().mockRejectedValue(new AuthError("guests_full", "x"));
    render(<AuthGate client={client(GUEST_ON, { guestStart })}>x</AuthGate>);
    fireEvent.click(await screen.findByRole("button", { name: "Continue as guest" }));
    fireEvent.click(await screen.findByRole("button", { name: "Accept and continue as guest" }));
    expect(await screen.findByText(/All guest places are taken/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to sign in" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
  });

  it("a guest session that ends returns to the login page with a notice", async () => {
    render(
      <AuthGate client={client(guestStatus())}>
        <Probe />
      </AuthGate>,
    );
    await screen.findByText(/app Guest/);
    act(() => notifyUnauthorized());
    expect(await screen.findByText(/guest session has ended/)).toBeInTheDocument();
    // The guest offer stays available.
    expect(screen.getByRole("button", { name: "Continue as guest" })).toBeInTheDocument();
  });
});

describe("forced password change", () => {
  const MUST = { ...accountStatus("listener", "newbie", 9), mustChangePassword: true };

  it("blocks the app until the one-time password is replaced", async () => {
    const changePassword = vi.fn().mockResolvedValue(undefined);
    const c = client([MUST, accountStatus("listener", "newbie", 9)], { changePassword });
    render(
      <AuthGate client={c}>
        <Probe />
      </AuthGate>,
    );
    expect(
      await screen.findByRole("heading", { name: "Choose a new password" }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/^app /)).toBeNull();
    fireEvent.submit(screen.getByLabelText("New password", { selector: "input" }).closest("form")!);
    expect(
      await screen.findByText("Enter the one-time password you were given."),
    ).toBeInTheDocument();
    expect(changePassword).not.toHaveBeenCalled();
    type("One-time password", "one time password words");
    type("New password", "my own long passphrase");
    type("Confirm new password", "my own long passphrase");
    fireEvent.submit(screen.getByLabelText("New password", { selector: "input" }).closest("form")!);
    await screen.findByText("app newbie listener listens");
    expect(changePassword).toHaveBeenCalledWith({
      currentPassword: "one time password words",
      newPassword: "my own long passphrase",
    });
  });

  it("refuses to reuse the one-time password and reports a wrong one", async () => {
    const changePassword = vi.fn().mockRejectedValue(new AuthError("invalid_credentials", "x"));
    render(<AuthGate client={client(MUST, { changePassword })}>x</AuthGate>);
    await screen.findByRole("heading", { name: "Choose a new password" });
    type("One-time password", "same-password-123");
    type("New password", "same-password-123");
    type("Confirm new password", "same-password-123");
    fireEvent.submit(screen.getByLabelText("New password", { selector: "input" }).closest("form")!);
    expect(await screen.findByText("Choose a different password.")).toBeInTheDocument();
    type("New password", "another-password-456");
    type("Confirm new password", "another-password-456");
    fireEvent.submit(screen.getByLabelText("New password", { selector: "input" }).closest("form")!);
    expect(await screen.findByText("The one-time password is not correct.")).toBeInTheDocument();
  });

  it("a data route answering password_change_required sends the user here", async () => {
    const c = client([
      accountStatus("operator"),
      { ...accountStatus("operator"), mustChangePassword: true },
    ]);
    render(
      <AuthGate client={c}>
        <Probe />
      </AuthGate>,
    );
    await screen.findByText(/app alice operator/);
    act(() => requestRecheck());
    expect(
      await screen.findByRole("heading", { name: "Choose a new password" }),
    ).toBeInTheDocument();
  });
});

describe("invitation page", () => {
  const preview = { role: "operator" as const, expiresAtMs: Date.now() + 1e6, invitedBy: "benny" };

  it("accepts an invitation and lands in the app, dropping the token from the URL", async () => {
    window.history.replaceState(null, "", "/invite/tok123");
    const acceptInvite = vi.fn().mockResolvedValue(undefined);
    const previewInvite = vi.fn().mockResolvedValue(preview);
    const c = client([makeStatus(), accountStatus("operator", "carol", 11)], {
      acceptInvite,
      previewInvite,
    });
    render(
      <AuthGate client={c}>
        <Probe />
      </AuthGate>,
    );
    expect(await screen.findByRole("heading", { name: "Join Superhet" })).toBeInTheDocument();
    expect(previewInvite).toHaveBeenCalledWith("tok123");
    expect(screen.getByText("Operator")).toBeInTheDocument();
    expect(screen.getByText(/benny invited you/)).toBeInTheDocument();
    type("Username", "carol");
    type("Password", "a long enough passphrase");
    type("Confirm password", "a long enough passphrase");
    fireEvent.submit(screen.getByLabelText("Username").closest("form")!);
    await screen.findByText("app carol operator tunes");
    expect(acceptInvite).toHaveBeenCalledWith({
      token: "tok123",
      username: "carol",
      password: "a long enough passphrase",
    });
    expect(window.location.pathname).toBe("/");
  });

  it("validates before calling the API and shows a taken username on the field", async () => {
    window.history.replaceState(null, "", "/invite/tok");
    const acceptInvite = vi.fn().mockRejectedValue(new AuthError("username_taken", "x"));
    render(
      <AuthGate
        client={client(makeStatus(), {
          acceptInvite,
          previewInvite: vi.fn().mockResolvedValue(preview),
        })}
      >
        x
      </AuthGate>,
    );
    await screen.findByRole("heading", { name: "Join Superhet" });
    fireEvent.submit(screen.getByLabelText("Username").closest("form")!);
    expect(await screen.findByText(/letters, digits/)).toBeInTheDocument();
    expect(acceptInvite).not.toHaveBeenCalled();
    type("Username", "alice");
    type("Password", "a long enough passphrase");
    type("Confirm password", "a long enough passphrase");
    fireEvent.submit(screen.getByLabelText("Username").closest("form")!);
    expect(
      await screen.findByText("This username is already taken. Pick another one."),
    ).toBeInTheDocument();
  });

  it("reports an unknown, used or expired link", async () => {
    window.history.replaceState(null, "", "/invite/dead");
    const previewInvite = vi.fn().mockRejectedValue(new AuthError("invalid_invite", "x"));
    render(<AuthGate client={client(makeStatus(), { previewInvite })}>x</AuthGate>);
    expect(
      await screen.findByRole("heading", { name: "Invitation not valid" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Go to sign in" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/");
  });

  it("warns when somebody is already signed in on this browser", async () => {
    window.history.replaceState(null, "", "/invite/tok");
    render(
      <AuthGate
        client={client(accountStatus("admin", "benny"), {
          previewInvite: vi.fn().mockResolvedValue(preview),
        })}
      >
        x
      </AuthGate>,
    );
    expect(await screen.findByText(/signed in as benny/)).toBeInTheDocument();
  });
});

describe("role changes while the page runs", () => {
  it("keeps the role from the status", async () => {
    render(
      <AuthGate client={client(accountStatus("listener", "lena", 4))}>
        <Probe />
      </AuthGate>,
    );
    expect(await screen.findByText("app lena listener listens")).toBeInTheDocument();
  });

  it("a setup-required server shows the wizard even on an invite path", async () => {
    window.history.replaceState(null, "", "/invite/tok");
    render(<AuthGate client={client(makeStatus({ setupRequired: true }))}>x</AuthGate>);
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Set up Superhet" })).toBeInTheDocument(),
    );
  });
});
