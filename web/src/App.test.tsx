import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { AuthContext } from "./auth/context";
import { accountStatus, authValue, stubClient } from "./auth/testing";
import { INSPECTOR_STORAGE_KEY } from "./shell/inspector";

beforeEach(() => window.localStorage.removeItem(INSPECTOR_STORAGE_KEY));

it("renders the shell: top bar, tuning strip, VFO row, spectrum, inspector and status bar", () => {
  render(<App />);
  expect(screen.getByText("Superhet")).toBeInTheDocument();
  // The top bar carries no frequency; the strip's create action is the only tuning entry point.
  expect(screen.queryByRole("group", { name: "Centre frequency" })).toBeNull();
  expect(screen.getByRole("region", { name: "Tuning" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Create receiver" })).toBeInTheDocument();
  expect(screen.getByRole("group", { name: "VFOs" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add VFO" })).toBeInTheDocument();
  expect(screen.getByRole("complementary", { name: "Inspector" })).toBeInTheDocument();
  expect(screen.getByRole("slider", { name: "Master volume" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /Audio (on|off)/ })).toBeInTheDocument();
  expect(screen.getByRole("contentinfo", { name: "Status" })).toBeInTheDocument();
});

it("the inspector tabs reach every former sidebar section", () => {
  render(<App />);
  const tabs = within(screen.getByRole("tablist", { name: "Inspector" }));
  expect(tabs.getAllByRole("tab").map((t) => t.textContent)).toEqual([
    "Receiver",
    "Bookmarks",
    "Channels",
    "Decoders",
    "Display",
    "Source",
  ]);
  expect(tabs.getByRole("tab", { name: "Receiver" })).toHaveAttribute("aria-selected", "true");
  fireEvent.click(tabs.getByRole("tab", { name: "Display" }));
  expect(screen.getByRole("button", { name: "Levels and zoom" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Band plan" })).toBeInTheDocument();
  expect(screen.getByRole("switch", { name: "Automatic levels" })).toBeInTheDocument();
  fireEvent.click(tabs.getByRole("tab", { name: "Source" }));
  for (const name of ["Source", "Profiles", "Audio device"]) {
    expect(screen.getByRole("button", { name })).toBeInTheDocument();
  }
  expect(screen.getByLabelText("Centre frequency")).toBeInTheDocument();
  fireEvent.click(tabs.getByRole("tab", { name: "Bookmarks" }));
  expect(tabs.getByRole("tab", { name: "Bookmarks" })).toHaveAttribute("aria-selected", "true");
  // Arrow keys move between tabs.
  fireEvent.keyDown(tabs.getByRole("tab", { name: "Bookmarks" }), { key: "ArrowRight" });
  expect(tabs.getByRole("tab", { name: "Channels" })).toHaveAttribute("aria-selected", "true");
});

it("] and the top bar button hide and show the inspector, and the choice persists", () => {
  const { unmount } = render(<App />);
  expect(screen.getByRole("tablist", { name: "Inspector" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "]" });
  expect(screen.queryByRole("tablist", { name: "Inspector" })).toBeNull();
  expect(window.localStorage.getItem(INSPECTOR_STORAGE_KEY)).toBe("0");
  unmount();
  render(<App />);
  expect(screen.queryByRole("tablist", { name: "Inspector" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Show inspector" }));
  expect(screen.getByRole("tablist", { name: "Inspector" })).toBeInTheDocument();
  expect(window.localStorage.getItem(INSPECTOR_STORAGE_KEY)).toBeNull();
});

it("] is ignored while typing in a field", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("tab", { name: "Source" }));
  fireEvent.keyDown(screen.getByLabelText("Centre frequency"), { key: "]" });
  expect(screen.getByRole("tablist", { name: "Inspector" })).toBeInTheDocument();
});

it("lists the inspector shortcut in the shortcuts dialog", () => {
  render(<App />);
  fireEvent.keyDown(window, { key: "?" });
  expect(screen.getByText("Show / hide the inspector")).toBeInTheDocument();
});

it("switches the theme from the settings menu", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});

it("shows the account section and signs out when rendered inside a session", () => {
  const signOut = vi.fn().mockResolvedValue(undefined);
  const client = stubClient({ changePassword: vi.fn().mockResolvedValue(undefined) });
  render(
    <AuthContext value={authValue(accountStatus("admin", "benny", 1), { client, signOut })}>
      <App />
    </AuthContext>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByTestId("signed-in-user")).toHaveTextContent("benny");
  fireEvent.click(screen.getByRole("button", { name: "Change password" }));
  expect(screen.getByRole("dialog", { name: "Change password" })).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Current password", { selector: "input" }), {
    target: { value: "old-password-123" },
  });
  fireEvent.change(screen.getByLabelText("New password", { selector: "input" }), {
    target: { value: "brand-new-password" },
  });
  fireEvent.change(screen.getByLabelText("Confirm new password", { selector: "input" }), {
    target: { value: "brand-new-password" },
  });
  fireEvent.submit(screen.getByLabelText("New password", { selector: "input" }).closest("form")!);
  return screen.findByText("Your password was changed.").then(() => {
    expect(client.changePassword).toHaveBeenCalledWith({
      currentPassword: "old-password-123",
      newPassword: "brand-new-password",
    });
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
    expect(signOut).toHaveBeenCalledTimes(1);
  });
});
