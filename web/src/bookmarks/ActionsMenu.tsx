import { useEffect, useRef, useState, type ReactNode } from "react";
import { IconButton, PlainButton, cn } from "../ui";

export interface MenuAction {
  label: string;
  onSelect: () => void;
  disabled?: boolean;
  /** Renders as a checkbox item (menuitemcheckbox). */
  checked?: boolean;
  /** A thin divider is drawn above this item. */
  separated?: boolean;
}

interface Props {
  label: string;
  icon: ReactNode;
  actions: readonly MenuAction[];
}

/** Icon button with a dropdown of actions; Esc and outside clicks close it. */
export function ActionsMenu({ label, icon, actions }: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (root.current && e.target instanceof Node && !root.current.contains(e.target)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  return (
    <div ref={root} className="relative">
      <IconButton
        label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        {icon}
      </IconButton>
      {open && (
        <div
          role="menu"
          aria-label={label}
          className="absolute top-full right-0 z-30 mt-1 min-w-48 rounded-lg border border-border-strong bg-surface-1 p-1 shadow-lg"
        >
          {actions.map((a) => (
            <div key={a.label}>
              {a.separated && <div role="separator" className="my-1 h-px bg-border" />}
              <PlainButton
                role={a.checked === undefined ? "menuitem" : "menuitemcheckbox"}
                aria-checked={a.checked}
                disabled={a.disabled}
                className={cn(
                  "flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-sm",
                  "text-fg hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-45",
                )}
                onClick={() => {
                  setOpen(false);
                  a.onSelect();
                }}
              >
                <span aria-hidden="true" className="w-3 text-accent">
                  {a.checked ? "✓" : ""}
                </span>
                {a.label}
              </PlainButton>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
