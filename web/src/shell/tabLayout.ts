// How the inspector tab strip spends its width: labels on every tab, a label on the selected tab
// only, or icons alone. Pure so the thresholds are testable without a layout engine.

export type TabMode = "full" | "compact" | "icons";

/** Approximate pixels of one 12 px semibold label character; icon (16) and padding/gap are fixed. */
const CHAR_PX = 6.6;
const ICON_TAB_PX = 32;
const LABEL_EXTRA_PX = 18;

/** Width a tab needs for the given mode (rough: the strip also reacts to real overflow). */
export function tabWidth(label: string, showLabel: boolean): number {
  return showLabel ? ICON_TAB_PX + LABEL_EXTRA_PX + label.length * CHAR_PX : ICON_TAB_PX;
}

/**
 * Pick the richest mode that fits `width`: all labels, else only the selected tab's label, else
 * icons only (every tab keeps its name for assistive tech and as a tooltip).
 */
export function chooseTabMode(width: number, labels: readonly string[], selected: number): TabMode {
  if (width <= 0) return "full";
  const full = labels.reduce((sum, l) => sum + tabWidth(l, true), 0);
  if (full <= width) return "full";
  const compact = labels.reduce((sum, l, i) => sum + tabWidth(l, i === selected), 0);
  return compact <= width ? "compact" : "icons";
}

/** Whether a scrollable strip hides content on either side (drives the scroll arrows). */
export function scrollHints(
  scrollLeft: number,
  clientWidth: number,
  scrollWidth: number,
): { left: boolean; right: boolean } {
  return {
    left: scrollLeft > 1,
    right: scrollLeft + clientWidth < scrollWidth - 1,
  };
}
