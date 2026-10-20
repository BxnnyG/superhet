import type { ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** Selected state (aria-pressed). */
  selected: boolean;
}

/** Full-width selectable list row (e.g. a VFO in the sidebar list). */
export function RowButton({ selected, className, children, ...rest }: Props) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      className={cn(
        "focus-ring flex h-9 min-w-0 items-center gap-2 rounded-md border px-2.5 text-left",
        "transition-colors duration-150",
        selected ? "border-accent/50 bg-accent-soft" : "border-transparent hover:bg-surface-2",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
