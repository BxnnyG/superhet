import { useRef, type KeyboardEvent } from "react";
import { cn } from "./cn";

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

interface Props<T extends string> {
  options: readonly SegmentOption<T>[];
  value: T;
  onChange: (value: T) => void;
  "aria-label": string;
  size?: "sm" | "md";
  className?: string;
}

/** Radio group rendered as a pill-shaped segmented control; arrow keys move the selection. */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  "aria-label": ariaLabel,
  size = "md",
  className,
}: Props<T>) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const onKeyDown = (e: KeyboardEvent, index: number) => {
    const dir =
      e.key === "ArrowRight" || e.key === "ArrowDown"
        ? 1
        : e.key === "ArrowLeft" || e.key === "ArrowUp"
          ? -1
          : 0;
    if (dir === 0) return;
    e.preventDefault();
    for (let step = 1; step <= options.length; step++) {
      const next = (index + dir * step + options.length * step) % options.length;
      const opt = options[next];
      if (opt && !opt.disabled) {
        onChange(opt.value);
        refs.current[next]?.focus();
        return;
      }
    }
  };

  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className={cn("inline-flex rounded-lg bg-surface-2 p-0.5", className)}
    >
      {options.map((o, i) => {
        const selected = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            disabled={o.disabled}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKeyDown(e, i)}
            className={cn(
              "focus-ring flex-1 rounded-md font-medium transition-colors duration-150",
              "disabled:cursor-not-allowed disabled:opacity-45",
              size === "sm" ? "h-6 px-2 text-xs" : "h-7 px-3 text-xs",
              selected
                ? "bg-surface-1 text-fg shadow-sm ring-1 ring-border"
                : "text-fg-muted hover:text-fg",
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
