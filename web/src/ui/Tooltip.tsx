import type { ReactNode } from "react";
import { cn } from "./cn";

interface Props {
  text: string;
  side?: "top" | "bottom";
  /** Anchor the bubble to the right edge (for controls at the container edge). */
  align?: "center" | "end";
  /** Extra classes for the wrapper (e.g. to make it block-level). */
  className?: string;
  children: ReactNode;
}

/**
 * CSS-only tooltip shown on hover and keyboard focus of the wrapped control. It is decorative
 * (aria-hidden): the wrapped control must carry its own accessible name.
 */
export function Tooltip({ text, side = "bottom", align = "center", className, children }: Props) {
  return (
    <span className={cn("group/tip relative inline-flex", className)}>
      {children}
      <span
        aria-hidden="true"
        className={cn(
          "pointer-events-none absolute z-50 whitespace-nowrap rounded-md",
          align === "end" ? "right-0" : "left-1/2 -translate-x-1/2",
          "bg-fg px-2 py-1 text-xs font-medium text-surface-0 opacity-0 shadow-pop",
          "transition-opacity delay-0 duration-150",
          "group-hover/tip:opacity-100 group-hover/tip:delay-500",
          "group-focus-within/tip:opacity-100",
          side === "bottom" ? "top-full mt-1.5" : "bottom-full mb-1.5",
        )}
      >
        {text}
      </span>
    </span>
  );
}
