import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthError, type AuthClient, type AuthStatus } from "./api";
import { AuthGate } from "./AuthGate";
import { useAuth } from "./context";
import { notifyUnauthorized } from "./session";
import { GUEST_LIMITS, accountStatus, makeStatus } from "./testing";

function fakeClient(status: AuthStatus, over: Partial<AuthClient> = {}): AuthClient {
  return {
    status: vi.fn().mockResolvedValue(status),
    setup: vi.fn().mockResolvedValue(undefined),
    login: vi.fn().mockResolvedValue(undefined),
    logout: vi.fn().mockResolvedValue(undefined),
    changePassword: vi.fn().mockResolvedValue(undefined),
    guestStart: vi.fn().mockResolvedValue(GUEST_LIMITS),
    previewInvite: vi.fn(),
    acceptInvite: vi.fn().mockResolvedValue(undefined),
    ...over,
  };
}

const SETUP: AuthStatus = makeStatus({ setupRequired: true });
const ANON: AuthStatus = makeStatus();
const AUTHED: AuthStatus = accountStatus("admin", "benny", 1);

function Probe() {
  const auth = useAuth();
  return (
    <div>
      <p>app for {auth?.username}</p>
      <button onClick={() => void auth?.signOut()}>out</button>
    </div>
  );
}

function type(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label, { selector: "input" }), { target: { value } });
}

describe("AuthGate routing", () => {
  it("shows the setup page when setup is required", async () => {
    render(<AuthGate client={fakeClient(SETUP)}>child</AuthGate>);
    expect(await screen.findByRole("heading", { name: "Set up Superhet" })).toBeInTheDocument();
    expect(screen.queryByText("child")).not.toBeInTheDocument();
  });

  it("shows the login page when signed out", async () => {
    render(<AuthGate client={fakeClient(ANON)}>child</AuthGate>);
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.getByLabelText("Username")).toHaveFocus();
    expect(screen.getByLabelText("Username")).toHaveAttribute("autocomplete", "username");
    expect(screen.getByLabelText("Password")).toHaveAttribute("autocomplete", "current-password");
  });

  it("renders the app when authenticated", async () => {
    render(
      <AuthGate client={fakeClient(AUTHED)}>
        <Probe />
      </AuthGate>,
    );
    expect(await screen.findByText("app for benny")).toBeInTheDocument();
  });

  it("offers a retry when the API is unreachable", async () => {
    const status = vi.fn().mockRejectedValueOnce(new AuthError("network", "x"));
    status.mockResolvedValue(ANON);
    render(<AuthGate client={fakeClient(ANON, { status })}>child</AuthGate>);
    fireEvent.click(await screen.findByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
  });

  it("explains a banned address instead of offering a retry", async () => {
    const status = vi.fn().mockRejectedValue(new AuthError("banned", "x"));
    render(<AuthGate client={fakeClient(ANON, { status })}>child</AuthGate>);
    expect(await screen.findByRole("heading", { name: "Access blocked" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  });

  it("signs in with Enter and lands in the app", async () => {
    const status = vi.fn().mockResolvedValueOnce(ANON).mockResolvedValue(AUTHED);
    const client = fakeClient(ANON, { status });
    render(
      <AuthGate client={client}>
        <Probe />
      </AuthGate>,
    );
    await screen.findByRole("heading", { name: "Sign in" });
    type("Username", "benny");
    type("Password", "hunter2hunter2");
    fireEvent.submit(screen.getByLabelText("Username").closest("form")!);
    expect(await screen.findByText("app for benny")).toBeInTheDocument();
    expect(client.login).toHaveBeenCalledWith({ username: "benny", password: "hunter2hunter2" });
  });
});

describe("login errors", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  async function openLogin(login: AuthClient["login"]) {
    render(<AuthGate client={fakeClient(ANON, { login })}>child</AuthGate>);
    await screen.findByRole("heading", { name: "Sign in" });
    type("Username", "benny");
    type("Password", "wrong-password");
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
  }

  it("shows invalid credentials and clears the password", async () => {
    await openLogin(vi.fn().mockRejectedValue(new AuthError("invalid_credentials", "x")));
    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid username or password.");
    expect(screen.getByLabelText("Password", { selector: "input" })).toHaveValue("");
    expect(screen.getByRole("button", { name: "Sign in" })).toBeEnabled();
  });

  it("disables the form with a countdown when rate limited", async () => {
    await openLogin(vi.fn().mockRejectedValue(new AuthError("rate_limited", "x", 3)));
    const btn = await screen.findByRole("button", { name: "Try again in 3s" });
    expect(btn).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("Too many sign-in attempts");
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(screen.getByRole("button", { name: "Try again in 2s" })).toBeDisabled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(screen.getByRole("button", { name: "Sign in" })).toBeEnabled();
  });

  it("formats long cooldowns as m:ss", async () => {
    await openLogin(vi.fn().mockRejectedValue(new AuthError("rate_limited", "x", 885)));
    expect(await screen.findByRole("button", { name: "Try again in 14:45" })).toBeDisabled();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(screen.getByRole("button", { name: "Try again in 14:44" })).toBeDisabled();
  });

  it("shows a loading state while the request is pending", async () => {
    let resolve: () => void = () => undefined;
    const login = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    await openLogin(login);
    expect(await screen.findByRole("button", { name: "Signing in…" })).toBeDisabled();
    await act(async () => resolve());
  });
});

describe("setup page", () => {
  async function openSetup(client = fakeClient(SETUP)) {
    render(<AuthGate client={client}>child</AuthGate>);
    await screen.findByRole("heading", { name: "Set up Superhet" });
    return client;
  }
  const submit = () => fireEvent.click(screen.getByRole("button", { name: "Create account" }));

  it("validates username, length, confirmation and token before calling the API", async () => {
    const client = await openSetup();
    type("Username", "bad name!");
    type("Password", "short");
    type("Confirm password", "other");
    submit();
    expect(screen.getByText(/1 to 64 characters/)).toBeInTheDocument();
    expect(screen.getByText("Use at least 12 characters.")).toBeInTheDocument();
    expect(screen.getByText("The passwords do not match.")).toBeInTheDocument();
    expect(screen.getByText(/Enter the setup token/)).toBeInTheDocument();
    expect(client.setup).not.toHaveBeenCalled();
  });

  it("has proper autocomplete attributes and explains the token", async () => {
    await openSetup();
    expect(screen.getByLabelText("Username")).toHaveAttribute("autocomplete", "username");
    expect(screen.getByLabelText("Password")).toHaveAttribute("autocomplete", "new-password");
    expect(screen.getByLabelText("Confirm password")).toHaveAttribute(
      "autocomplete",
      "new-password",
    );
    expect(screen.getByText(/docker compose logs superhet-api/)).toBeInTheDocument();
  });

  it("shows a strength hint while typing", async () => {
    await openSetup();
    type("Password", "correct horse battery staple 42");
    expect(screen.getByTestId("strength")).toHaveTextContent("Strength: Strong");
  });

  it("toggles password visibility", async () => {
    await openSetup();
    const field = screen.getByLabelText("Password", { selector: "input" });
    expect(field).toHaveAttribute("type", "password");
    fireEvent.click(screen.getAllByRole("button", { name: "Show password" })[0]!);
    expect(field).toHaveAttribute("type", "text");
  });

  it("submits valid data and enters the app", async () => {
    const status = vi.fn().mockResolvedValueOnce(SETUP).mockResolvedValue(AUTHED);
    const client = await openSetup(fakeClient(SETUP, { status }));
    type("Username", "benny");
    type("Password", "a-long-enough-pass");
    type("Confirm password", "a-long-enough-pass");
    type("Setup token", " tok-123 ");
    submit();
    await waitFor(() => expect(screen.getByText("child")).toBeInTheDocument());
    expect(client.setup).toHaveBeenCalledWith({
      username: "benny",
      password: "a-long-enough-pass",
      setupToken: "tok-123",
    });
  });

  it("flags a wrong setup token on the field", async () => {
    const client = fakeClient(SETUP, {
      setup: vi.fn().mockRejectedValue(new AuthError("invalid_setup_token", "x")),
    });
    await openSetup(client);
    type("Username", "benny");
    type("Password", "a-long-enough-pass");
    type("Confirm password", "a-long-enough-pass");
    type("Setup token", "nope");
    submit();
    expect(await screen.findByText(/setup token is not valid/)).toBeInTheDocument();
  });

  it("sends the user to login when setup already happened", async () => {
    const client = fakeClient(SETUP, {
      setup: vi.fn().mockRejectedValue(new AuthError("already_setup", "x")),
    });
    await openSetup(client);
    type("Username", "benny");
    type("Password", "a-long-enough-pass");
    type("Confirm password", "a-long-enough-pass");
    type("Setup token", "t");
    submit();
    fireEvent.click(await screen.findByRole("button", { name: "Go to sign in" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
  });
});

describe("session handling in the app", () => {
  it("returns to login with a notice after a 401 anywhere", async () => {
    render(
      <AuthGate client={fakeClient(AUTHED)}>
        <Probe />
      </AuthGate>,
    );
    await screen.findByText("app for benny");
    act(() => notifyUnauthorized());
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Your session has ended");
    expect(screen.queryByText("app for benny")).not.toBeInTheDocument();
  });

  it("signs out through the server and shows login", async () => {
    const client = fakeClient(AUTHED);
    render(
      <AuthGate client={client}>
        <Probe />
      </AuthGate>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "out" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
    expect(client.logout).toHaveBeenCalledTimes(1);
  });

  it("signs out locally even when the logout request fails", async () => {
    const client = fakeClient(AUTHED, { logout: vi.fn().mockRejectedValue(new Error("down")) });
    render(
      <AuthGate client={client}>
        <Probe />
      </AuthGate>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "out" }));
    expect(await screen.findByRole("heading", { name: "Sign in" })).toBeInTheDocument();
  });
});
