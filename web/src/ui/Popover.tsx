import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { cn } from "./cn";

interface TriggerProps {
  open: boolean;
  /** Spread onto the trigger button. */
  props: {
    "aria-expanded": boolean;
    "aria-haspopup": "dialog";
    "aria-controls": string | undefined;
    onClick: () => void;
  };
}

interface Props {
  /** Accessible name of the popover dialog. */
  label: string;
  trigger: (t: TriggerProps) => ReactNode;
  children: ReactNode;
  align?: "start" | "end";
  className?: string;
  /** Classes of the panel (width, padding). */
  panelClassName?: string;
}

/** Anchored popover: closes on Escape or a click outside; content stays mounted only while open. */
export function Popover({
  label,
  trigger,
  children,
  align = "end",
  className,
  panelClassName,
}: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [open]);

  return (
    <div ref={rootRef} className={cn("relative", className)}>
      {trigger({
        open,
        props: {
          "aria-expanded": open,
          "aria-haspopup": "dialog",
          "aria-controls": open ? panelId : undefined,
          onClick: () => setOpen((o) => !o),
        },
      })}
      {open && (
        <div
          id={panelId}
          role="dialog"
          aria-label={label}
          className={cn(
            "absolute top-full z-50 mt-2 max-h-[calc(100dvh-8rem)] w-80 max-w-[calc(100vw-1.5rem)] overflow-y-auto rounded-lg border border-border bg-surface-1 p-3 shadow-pop",
            align === "end" ? "right-0" : "left-0",
            panelClassName,
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}
