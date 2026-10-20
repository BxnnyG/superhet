import { cn } from "./cn";

interface Props {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  hideLabel?: boolean;
  disabled?: boolean;
  className?: string;
}

/** Switch (role="switch"); Space/Enter toggle. */
export function Toggle({ checked, onChange, label, hideLabel, disabled, className }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={hideLabel ? label : undefined}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "focus-ring group inline-flex items-center gap-2 rounded-md text-sm text-fg",
        "disabled:cursor-not-allowed disabled:opacity-45",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "relative inline-block h-5 w-9 shrink-0 rounded-full transition-colors duration-150",
          checked ? "bg-accent" : "bg-surface-3",
        )}
      >
        <span
          className={cn(
            "absolute top-0.5 left-0.5 size-4 rounded-full bg-white shadow-sm",
            "transition-transform duration-150",
            checked && "translate-x-4",
          )}
        />
      </span>
      {!hideLabel && <span>{label}</span>}
    </button>
  );
}
