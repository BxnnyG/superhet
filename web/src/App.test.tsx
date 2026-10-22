import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { App } from "./App";
import { AuthContext } from "./auth/context";

it("renders the SDR shell: top bar, sidebar sections, spectrum, receiver dock and status bar", () => {
  render(<App />);
  expect(screen.getByText("Superhet")).toBeInTheDocument();
  expect(screen.getByRole("group", { name: "Centre frequency" })).toBeInTheDocument();
  expect(screen.getByRole("complementary", { name: "Controls" })).toBeInTheDocument();
  for (const name of ["Source", "Profiles", "VFOs", "Display", "Audio device"]) {
    expect(screen.getByRole("button", { name })).toBeInTheDocument();
  }
  expect(screen.getByRole("region", { name: "Receiver" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Add VFO" })).toBeInTheDocument();
  expect(screen.getByRole("contentinfo", { name: "Status" })).toBeInTheDocument();
});

it("opens and closes the drawer from the menu button", () => {
  render(<App />);
  const menu = screen.getByRole("button", { name: "Open controls" });
  expect(menu).toHaveAttribute("aria-expanded", "false");
  fireEvent.click(menu);
  expect(menu).toHaveAttribute("aria-expanded", "true");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(menu).toHaveAttribute("aria-expanded", "false");
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
  const client = {
    status: vi.fn(),
    setup: vi.fn(),
    login: vi.fn(),
    logout: vi.fn(),
    changePassword: vi.fn().mockResolvedValue(undefined),
  };
  render(
    <AuthContext value={{ username: "benny", client, signOut }}>
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
