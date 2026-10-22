import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Toaster } from "./Toast";
import { ToastStore } from "./toast";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

it("shows, replaces by key and auto-dismisses toasts", () => {
  const store = new ToastStore();
  render(<Toaster store={store} />);
  act(() => {
    store.push({ key: "locked", tone: "warn", title: "Tuning is locked", body: "Locked by you." });
    store.push({ key: "locked", tone: "warn", title: "Tuning is locked", body: "Again" });
  });
  expect(screen.getAllByText("Tuning is locked")).toHaveLength(1);
  expect(screen.getByText("Again")).toBeInTheDocument();
  act(() => {
    vi.advanceTimersByTime(6_000);
  });
  expect(screen.queryByText("Tuning is locked")).toBeNull();
});

it("dismisses on click and announces danger as alert", () => {
  const store = new ToastStore();
  render(<Toaster store={store} />);
  act(() => {
    store.push({ tone: "danger", title: "Boom" });
    store.push({ tone: "info", title: "Fyi", durationMs: 0 });
  });
  expect(screen.getByRole("alert")).toHaveTextContent("Boom");
  expect(screen.getAllByRole("status")).toHaveLength(1);
  fireEvent.click(screen.getAllByRole("button", { name: "Dismiss notification" })[1]!);
  expect(screen.queryByText("Fyi")).toBeNull();
});

it("keeps at most four toasts", () => {
  const store = new ToastStore();
  for (let i = 0; i < 6; i++) store.push({ title: `t${i}`, durationMs: 0 });
  expect(store.getSnapshot().map((t) => t.title)).toEqual(["t2", "t3", "t4", "t5"]);
});
