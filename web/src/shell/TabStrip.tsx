import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { ChevronLeftIcon, ChevronRightIcon, PlainButton, Tooltip, cn } from "../ui";
import { chooseTabMode, scrollHints } from "./tabLayout";

export interface TabItem<T extends string> {
  id: T;
  label: string;
  icon: ReactNode;
}

interface Props<T extends string> {
  tabs: readonly TabItem<T>[];
  selected: T;
  onSelect: (id: T) => void;
  tabId: (id: T) => string;
  panelId: string;
  label: string;
}

/**
 * Tab strip that always fits: icon + label while the width allows, then only the selected tab
 * keeps its label, then icons with tooltips. Should even that overflow (very narrow sheets, many
 * tabs) the strip scrolls and shows arrow buttons. Roving tabindex, Arrow/Home/End keys.
 */
export function TabStrip<T extends string>({
  tabs,
  selected,
  onSelect,
  tabId,
  panelId,
  label,
}: Props<T>) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const refs = useRef<Partial<Record<string, HTMLButtonElement | null>>>({});
  const [width, setWidth] = useState(0);
  const [hints, setHints] = useState({ left: false, right: false });

  const labels = tabs.map((t) => t.label);
  const selectedIndex = Math.max(
    0,
    tabs.findIndex((t) => t.id === selected),
  );
  const mode = chooseTabMode(width, labels, selectedIndex);

  const measure = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const next = scrollHints(el.scrollLeft, el.clientWidth, el.scrollWidth);
    setHints((prev) => (prev.left === next.left && prev.right === next.right ? prev : next));
  }, []);

  useLayoutEffect(() => {
    measure();
    const el = listRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [measure]);
  // The mode (and so the content width) changes with the selection and the tab set.
  useEffect(measure, [measure, mode, selected, tabs.length]);

  // Keep the selected tab visible when the strip has to scroll.
  useEffect(() => {
    refs.current[selected]?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected, mode]);

  const onKey = (e: KeyboardEvent, index: number) => {
    const delta = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (delta === 0 && e.key !== "Home" && e.key !== "End") return;
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? tabs.length - 1
          : (index + delta + tabs.length) % tabs.length;
    e.preventDefault();
    const target = tabs[next];
    if (!target) return;
    onSelect(target.id);
    refs.current[target.id]?.focus();
  };

  const scrollBy = (dir: -1 | 1) => {
    const el = listRef.current;
    el?.scrollBy?.({ left: dir * Math.max(80, el.clientWidth * 0.6), behavior: "smooth" });
  };

  return (
    <div className="relative flex min-w-0 flex-1 items-stretch self-stretch">
      {hints.left && <ScrollArrow dir={-1} onClick={() => scrollBy(-1)} />}
      <div
        ref={listRef}
        role="tablist"
        aria-label={label}
        onScroll={measure}
        data-mode={mode}
        className="no-scrollbar flex min-w-0 flex-1 items-stretch overflow-x-auto px-1"
      >
        {tabs.map((t, i) => {
          const active = t.id === selected;
          const showLabel = mode === "full" || (mode === "compact" && active);
          const button = (
            <PlainButton
              ref={(el) => {
                refs.current[t.id] = el;
              }}
              role="tab"
              id={tabId(t.id)}
              aria-selected={active}
              aria-controls={panelId}
              tabIndex={active ? 0 : -1}
              onClick={() => onSelect(t.id)}
              onKeyDown={(e) => onKey(e, i)}
              className={cn(
                "relative flex shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-medium",
                "transition-colors duration-150",
                active ? "text-fg" : "text-fg-subtle hover:text-fg",
                // Underline of the selected tab.
                active &&
                  "after:absolute after:inset-x-1.5 after:bottom-0 after:h-0.5 after:rounded-full after:bg-accent",
              )}
            >
              {t.icon}
              <span className={showLabel ? undefined : "sr-only"}>{t.label}</span>
            </PlainButton>
          );
          return showLabel ? (
            <span key={t.id} className="flex self-stretch">
              {button}
            </span>
          ) : (
            <Tooltip key={t.id} text={t.label} className="self-stretch">
              {button}
            </Tooltip>
          );
        })}
      </div>
      {hints.right && <ScrollArrow dir={1} onClick={() => scrollBy(1)} />}
    </div>
  );
}

function ScrollArrow({ dir, onClick }: { dir: -1 | 1; onClick: () => void }) {
  return (
    <PlainButton
      aria-label={dir < 0 ? "Scroll tabs left" : "Scroll tabs right"}
      tabIndex={-1}
      onClick={onClick}
      className="flex w-6 shrink-0 items-center justify-center text-fg-subtle hover:text-fg"
    >
      {dir < 0 ? <ChevronLeftIcon size={14} /> : <ChevronRightIcon size={14} />}
    </PlainButton>
  );
}
