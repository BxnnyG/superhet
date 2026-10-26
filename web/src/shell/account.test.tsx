import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "../App";
import type { SelfApi } from "../admin/api";
import { AuthContext } from "../auth/context";
import { MySessionsDialog } from "../auth/MySessionsDialog";
import { GUEST_LIMITS, accountStatus, authValue, guestStatus } from "../auth/testing";
import { setLockIdentity } from "../state/locks";
import { INSPECTOR_STORAGE_KEY } from "./inspector";

beforeEach(() => window.localStorage.removeItem(INSPECTOR_STORAGE_KEY));
afterEach(() => setLockIdentity(undefined));

function mount(value: ReturnType<typeof authValue>) {
  return render(
    <AuthContext value={value}>
      <App />
    </AuthContext>,
  );
}

function openSettings(value: ReturnType<typeof authValue>) {
  mount(value);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  return screen.getByRole("dialog", { name: "Settings" });
}

it("shows the role badge and the admin console entry to admins", () => {
  const dialog = openSettings(authValue(accountStatus("admin", "root", 1)));
  expect(within(dialog).getByTestId("signed-in-user")).toHaveTextContent("root");
  expect(within(dialog).getByText("Admin")).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "Admin console" })).toBeInTheDocument();
  expect(within(dialog).getByRole("button", { name: "My sessions" })).toBeInTheDocument();
});

it.each(["operator", "listener"] as const)("a %s gets no admin navigation", (role) => {
  const dialog = openSettings(authValue(accountStatus(role, "u", 2)));
  expect(within(dialog).queryByRole("button", { name: "Admin console" })).toBeNull();
  expect(within(dialog).getByRole("button", { name: "My sessions" })).toBeInTheDocument();
});

it("guests have no password, no sessions page and can end the session", () => {
  const dialog = openSettings(authValue(guestStatus(), { limits: GUEST_LIMITS }));
  expect(within(dialog).getAllByText("Guest")).toHaveLength(2); // name and role badge
  expect(within(dialog).queryByRole("button", { name: "Change password" })).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "My sessions" })).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "Admin console" })).toBeNull();
  expect(within(dialog).getByRole("button", { name: "End guest session" })).toBeInTheDocument();
});

it("guests have no bookmarks tab and no profiles", () => {
  mount(authValue(guestStatus(), { limits: GUEST_LIMITS }));
  expect(screen.queryByRole("tab", { name: "Bookmarks" })).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: "Source" }));
  expect(screen.queryByText("Profiles")).toBeNull();
});

it("listeners keep the bookmarks tab but not the profiles", () => {
  mount(authValue(accountStatus("listener")));
  expect(screen.getByRole("tab", { name: "Bookmarks" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("tab", { name: "Source" }));
  expect(screen.queryByText("Profiles")).toBeNull();
});

it("admins see the profiles section", () => {
  mount(authValue(accountStatus("admin")));
  fireEvent.click(screen.getByRole("tab", { name: "Source" }));
  expect(screen.getByText("Profiles")).toBeInTheDocument();
});

it("my sessions lists the devices and revokes another one", async () => {
  const api: SelfApi = {
    list: vi.fn().mockResolvedValue([
      {
        sid: "s-current",
        ip: "198.51.100.7",
        userAgent: "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
        createdAtMs: Date.now() - 60_000,
        lastSeenMs: Date.now(),
        current: true,
      },
      {
        sid: "s-phone",
        ip: null,
        userAgent: "Mozilla/5.0 (Linux; Android 14) Chrome/130.0 Mobile Safari/537.36",
        createdAtMs: Date.now() - 86_400_000,
        lastSeenMs: Date.now() - 3_600_000,
        current: false,
      },
    ]),
    revoke: vi.fn().mockResolvedValue(undefined),
  };
  render(<MySessionsDialog onClose={() => undefined} api={api} />);
  expect(await screen.findByText("Firefox on Linux")).toBeInTheDocument();
  expect(screen.getByText("Chrome on Android")).toBeInTheDocument();
  expect(screen.getByText("This device")).toBeInTheDocument();
  expect(screen.getByText("address not stored")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
  await waitFor(() => expect(api.revoke).toHaveBeenCalledWith("s-phone"));
});
