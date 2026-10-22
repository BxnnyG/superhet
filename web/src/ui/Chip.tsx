import type { ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** Selected state (aria-pressed). */
  selected: boolean;
  /** De-emphasised (e.g. a paused VFO). */
  muted?: boolean;
}

/** Compact selectable pill for choosing one item out of a short row (e.g. the active VFO). */
export function Chip({ selected, muted, className, children, ...rest }: Props) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      className={cn(
        "focus-ring inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-xs",
        "font-medium transition-colors duration-150",
        selected
          ? "border-accent/50 bg-accent-soft text-fg"
          : "border-border-strong bg-surface-1 text-fg-muted hover:bg-surface-2 hover:text-fg",
        muted && "italic opacity-70",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
