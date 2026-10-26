import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Kbd, SearchIcon } from "../ui";
import { cn } from "../ui/cn";
import { buildSections, flatten, type PaletteDeps, type PaletteItem } from "./items";

interface Props {
  deps: PaletteDeps;
  onClose: () => void;
}

/**
 * Command palette: a modal combobox. Focus stays in the search field (the only tab stop); the
 * arrow keys move `aria-activedescendant` through the grouped results, Enter runs the entry and
 * Alt+Enter opens a frequency or bookmark in a new VFO. Escape, a click outside or Ctrl/Cmd+K
 * (handled by the shortcut listener) close it and focus returns to where it was.
 */
export function Palette({ deps, onClose }: Props) {
  const base = useId();
  const listId = `${base}-list`;
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  });

  const sections = useMemo(() => buildSections(deps, query), [deps, query]);
  const flat = useMemo(() => flatten(sections), [sections]);
  const index = Math.min(active, Math.max(0, flat.length - 1));
  const current: PaletteItem | undefined = flat[index];
  const optionId = (item: PaletteItem) => `${base}-opt-${item.id}`;

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    inputRef.current?.focus();
    return () => previous?.focus();
  }, []);

  useEffect(() => {
    if (current) document.getElementById(optionId(current))?.scrollIntoView?.({ block: "nearest" });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only the active entry matters
  }, [current?.id]);

  // Close first so focus is restored, then run: an action may move focus itself (type a frequency).
  const choose = (item: PaletteItem, newVfo: boolean) => {
    onClose();
    window.setTimeout(() => item.run({ newVfo }), 0);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        e.stopPropagation();
        onClose();
        return;
      case "Tab":
        e.preventDefault(); // the field is the only stop; focus is trapped in the dialog
        return;
      case "ArrowDown":
      case "ArrowUp": {
        e.preventDefault();
        if (flat.length === 0) return;
        const step = e.key === "ArrowDown" ? 1 : -1;
        setActive((index + step + flat.length) % flat.length);
        return;
      }
      case "Enter":
        e.preventDefault();
        if (current && !e.nativeEvent.isComposing) choose(current, e.altKey && !!current.newVfoAlt);
        return;
    }
  };

  const count = flat.length;
  return createPortal(
    <div
      className="fixed inset-0 z-[70] flex items-start justify-center bg-overlay px-3 pt-[10vh]"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        data-palette=""
        onKeyDown={onKeyDown}
        className="flex max-h-[min(34rem,80dvh)] w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border-strong bg-surface-1 shadow-pop"
      >
        <div className="flex h-12 shrink-0 items-center gap-2.5 border-b border-border px-3.5">
          <SearchIcon size={16} className="shrink-0 text-fg-subtle" />
          <input
            ref={inputRef}
            type="text"
            role="combobox"
            aria-label="Search commands, bookmarks or type a frequency"
            aria-expanded="true"
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={current ? optionId(current) : undefined}
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            placeholder="Search or jump…  try 145.5m nfm"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
            className="h-full min-w-0 flex-1 bg-transparent text-sm text-fg outline-none placeholder:text-fg-subtle"
          />
          <Kbd>Esc</Kbd>
        </div>
        <div
          id={listId}
          role="listbox"
          aria-label="Results"
          className="min-h-0 flex-1 overflow-y-auto p-1.5"
        >
          {sections.map((section) => {
            const headingId = `${base}-h-${section.group}`;
            return (
              <div key={section.group} role="group" aria-labelledby={headingId}>
                <div
                  id={headingId}
                  className="px-2.5 pt-2 pb-1 text-[11px] font-medium tracking-wide text-fg-subtle uppercase"
                >
                  {section.group}
                </div>
                {section.items.map((item) => {
                  const selected = item === current;
                  return (
                    <div
                      key={item.id}
                      id={optionId(item)}
                      role="option"
                      aria-selected={selected}
                      onPointerMove={() => {
                        const at = flat.indexOf(item);
                        if (at !== index) setActive(at);
                      }}
                      onClick={(e) => choose(item, e.altKey && !!item.newVfoAlt)}
                      className={cn(
                        "flex min-h-10 cursor-pointer items-center gap-2.5 rounded-md px-2.5 py-1.5",
                        selected ? "bg-accent-soft" : "hover:bg-surface-2",
                      )}
                    >
                      {item.color && (
                        <span
                          aria-hidden="true"
                          className="size-2 shrink-0 rounded-full"
                          style={{ background: item.color }}
                        />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm text-fg">{item.title}</span>
                        {item.subtitle && (
                          <span className="block truncate text-xs text-fg-muted num">
                            {item.subtitle}
                          </span>
                        )}
                      </span>
                      {item.hint && (
                        <span aria-hidden="true" className="flex shrink-0 items-center gap-1">
                          {item.hint.map((k, i) => (
                            <Kbd key={`${k}-${i}`}>{k}</Kbd>
                          ))}
                        </span>
                      )}
                      {selected && item.newVfoAlt && (
                        <span
                          aria-hidden="true"
                          className="flex shrink-0 items-center gap-1 text-[11px] text-fg-subtle max-phone:hidden"
                        >
                          <Kbd>Alt ↵</Kbd> new VFO
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            );
          })}
          {count === 0 && (
            <p className="px-3 py-8 text-center text-sm text-fg-muted">
              Nothing matches “{query.trim()}”.
            </p>
          )}
        </div>
        <div
          aria-hidden="true"
          className="flex shrink-0 items-center gap-4 border-t border-border bg-surface-0/40 px-3.5 py-2 text-[11px] text-fg-subtle max-phone:hidden"
        >
          <span className="flex items-center gap-1">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> navigate
          </span>
          <span className="flex items-center gap-1">
            <Kbd>↵</Kbd> run
          </span>
          <span className="flex items-center gap-1">
            <Kbd>Alt ↵</Kbd> new VFO
          </span>
        </div>
        <p role="status" className="sr-only">
          {count === 0 ? "No results" : `${count} result${count === 1 ? "" : "s"}`}
        </p>
      </div>
    </div>,
    document.body,
  );
}
