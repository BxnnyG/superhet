import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { INSPECTOR_LABEL, INSPECTOR_TABS } from "./inspector";
import { tabIcon } from "./tabIcons";
import { chooseTabMode, scrollHints } from "./tabLayout";
import { TabStrip } from "./TabStrip";

const LABELS = INSPECTOR_TABS.map((t) => INSPECTOR_LABEL[t]);

describe("chooseTabMode", () => {
  it("keeps every label when there is room", () => {
    expect(chooseTabMode(900, LABELS, 0)).toBe("full");
  });

  it("the desktop inspector column (about 300 px) shows the selected label only", () => {
    expect(chooseTabMode(300, LABELS, 0)).toBe("compact");
  });

  it("narrow sheets fall back to icons", () => {
    expect(chooseTabMode(230, LABELS, 0)).toBe("icons");
    expect(chooseTabMode(180, LABELS, 3)).toBe("icons");
  });

  it("an unmeasured strip (width 0) renders in full", () => {
    expect(chooseTabMode(0, LABELS, 0)).toBe("full");
  });

  it("a longer selected label needs more room than a short one", () => {
    // Bookmarks (9 chars) is wider than Source (6): a width that suits Source may not suit it.
    const w = 290;
    expect(chooseTabMode(w, LABELS, 6)).toBe("compact");
    expect(chooseTabMode(w, LABELS, 1)).toBe("icons");
  });
});

describe("scrollHints", () => {
  it("shows arrows only on the sides with hidden content", () => {
    expect(scrollHints(0, 200, 500)).toEqual({ left: false, right: true });
    expect(scrollHints(150, 200, 500)).toEqual({ left: true, right: true });
    expect(scrollHints(300, 200, 500)).toEqual({ left: true, right: false });
    expect(scrollHints(0, 200, 200)).toEqual({ left: false, right: false });
  });
});

describe("TabStrip", () => {
  afterEach(() => vi.restoreAllMocks());

  function strip(selected: (typeof INSPECTOR_TABS)[number], onSelect = vi.fn()) {
    return render(
      <TabStrip
        label="Inspector"
        tabs={INSPECTOR_TABS.map((t) => ({
          id: t,
          label: INSPECTOR_LABEL[t],
          icon: tabIcon(t),
        }))}
        selected={selected}
        onSelect={onSelect}
        tabId={(t) => `tab-${t}`}
        panelId="panel"
      />,
    );
  }

  function width(px: number) {
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(px);
  }

  it("collapses unselected tabs to icons with a tooltip when narrow, names stay accessible", () => {
    width(300);
    strip("decoders");
    const list = screen.getByRole("tablist", { name: "Inspector" });
    expect(list).toHaveAttribute("data-mode", "compact");
    // The accessible name is the label in every mode.
    for (const l of LABELS) expect(screen.getByRole("tab", { name: l })).toBeInTheDocument();
    const decoders = screen.getByRole("tab", { name: "Decoders" });
    expect(decoders.querySelector("span")).not.toHaveClass("sr-only");
    const display = screen.getByRole("tab", { name: "Display" });
    expect(display.querySelector("span")).toHaveClass("sr-only");
    // Icon-only tabs carry a tooltip bubble.
    expect(display.parentElement?.textContent).toContain("Display");
  });

  it("icon-only below the compact threshold", () => {
    width(200);
    strip("receiver");
    expect(screen.getByRole("tablist")).toHaveAttribute("data-mode", "icons");
  });

  it("full labels on wide strips", () => {
    width(800);
    strip("receiver");
    expect(screen.getByRole("tablist")).toHaveAttribute("data-mode", "full");
  });

  it("arrow keys, Home and End move the selection with wrap-around", () => {
    width(300);
    const onSelect = vi.fn();
    strip("receiver", onSelect);
    const receiver = screen.getByRole("tab", { name: "Receiver" });
    fireEvent.keyDown(receiver, { key: "ArrowLeft" });
    expect(onSelect).toHaveBeenLastCalledWith("source");
    fireEvent.keyDown(receiver, { key: "ArrowRight" });
    expect(onSelect).toHaveBeenLastCalledWith("bookmarks");
    fireEvent.keyDown(receiver, { key: "End" });
    expect(onSelect).toHaveBeenLastCalledWith("source");
    fireEvent.keyDown(receiver, { key: "Home" });
    expect(onSelect).toHaveBeenLastCalledWith("receiver");
  });

  it("roving tabindex: only the selected tab is in the tab order", () => {
    width(300);
    strip("display");
    expect(screen.getByRole("tab", { name: "Display" })).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("tab", { name: "Source" })).toHaveAttribute("tabindex", "-1");
  });

  it("shows scroll arrows when content still overflows", () => {
    width(120);
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(300);
    strip("receiver");
    expect(screen.getByRole("button", { name: "Scroll tabs right" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Scroll tabs left" })).toBeNull();
  });
});
