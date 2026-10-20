import { useId } from "react";
import { cn } from "./cn";
import { ChevronDownIcon } from "./icons";

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface Props {
  options: readonly SelectOption[];
  value: string;
  onChange: (value: string) => void;
  /** Visible label above the control; without it `aria-label` is required. */
  label?: string;
  "aria-label"?: string;
  disabled?: boolean;
  size?: "sm" | "md";
  className?: string;
  title?: string;
}

/** Styled native select (keeps platform keyboard and mobile behaviour). */
export function Select({
  options,
  value,
  onChange,
  label,
  "aria-label": ariaLabel,
  disabled,
  size = "md",
  className,
  title,
}: Props) {
  const id = useId();
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      {label && (
        <label htmlFor={id} className="text-xs font-medium text-fg-muted">
          {label}
        </label>
      )}
      <div className="relative">
        <select
          id={id}
          aria-label={label ? undefined : ariaLabel}
          title={title}
          disabled={disabled}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className={cn(
            "focus-ring w-full appearance-none rounded-md border border-border-strong bg-surface-1",
            "pr-7 pl-2.5 text-fg num transition-colors duration-150 hover:bg-surface-2",
            "disabled:cursor-not-allowed disabled:opacity-45",
            size === "sm" ? "h-7 text-xs" : "h-8 text-sm",
          )}
        >
          {options.map((o) => (
            <option key={o.value} value={o.value} disabled={o.disabled}>
              {o.label}
            </option>
          ))}
        </select>
        <ChevronDownIcon
          size={14}
          className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-fg-subtle"
        />
      </div>
    </div>
  );
}
