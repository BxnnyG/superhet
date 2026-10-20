import { useId, useState } from "react";
import { cn } from "./cn";

interface Props {
  value: number;
  /** Called with the clamped value on Enter, blur or arrow keys. */
  onCommit: (value: number) => void;
  label: string;
  min?: number;
  max?: number;
  step?: number;
  /** Digits after the decimal point shown and accepted. */
  decimals?: number;
  unit?: string;
  /** Custom text -> value parser (null = invalid) and value -> text formatter. */
  parse?: (text: string) => number | null;
  format?: (value: number) => string;
  disabled?: boolean;
  className?: string;
}

export function clampNumber(v: number, min?: number, max?: number): number {
  return Math.min(max ?? Infinity, Math.max(min ?? -Infinity, v));
}

/** Text input for a number with unit suffix; ArrowUp/Down step, Enter/blur commit, Esc reverts. */
export function NumberField({
  value,
  onCommit,
  label,
  min,
  max,
  step = 1,
  decimals = 0,
  unit,
  parse,
  format,
  disabled,
  className,
}: Props) {
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const fmt = format ?? ((v: number) => v.toFixed(decimals));
  const read =
    parse ?? ((t: string) => (t.trim() === "" ? null : Number(t.trim().replace(",", "."))));
  const shown = draft ?? fmt(value);

  const reset = () => {
    setDraft(null);
    setInvalid(false);
  };

  const commit = (text: string): boolean => {
    const n = read(text);
    if (n === null || !Number.isFinite(n)) {
      setInvalid(true);
      return false;
    }
    onCommit(clampNumber(Number(n.toFixed(decimals)), min, max));
    reset();
    return true;
  };

  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <label htmlFor={id} className="text-xs font-medium text-fg-muted">
        {label}
      </label>
      <div
        className={cn(
          "flex h-8 items-center rounded-md border bg-surface-1 transition-colors duration-150",
          "focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/30",
          invalid ? "border-danger" : "border-border-strong",
          disabled && "opacity-45",
        )}
      >
        <input
          id={id}
          type="text"
          inputMode="decimal"
          spellCheck={false}
          disabled={disabled}
          aria-invalid={invalid}
          value={shown}
          onChange={(e) => {
            setDraft(e.target.value);
            setInvalid(false);
          }}
          onFocus={(e) => e.currentTarget.select()}
          onBlur={() => {
            if (draft !== null) commit(draft);
            reset();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              if (commit(draft ?? shown)) e.currentTarget.blur();
            } else if (e.key === "Escape") {
              reset();
              e.currentTarget.blur();
            } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
              e.preventDefault();
              const base = read(draft ?? shown);
              const cur = base !== null && Number.isFinite(base) ? base : value;
              const dir = e.key === "ArrowUp" ? 1 : -1;
              onCommit(clampNumber(Number((cur + dir * step).toFixed(decimals)), min, max));
              reset();
            }
          }}
          className="h-full min-w-0 flex-1 bg-transparent px-2.5 text-right font-mono text-sm text-fg num outline-none"
        />
        {unit && <span className="pr-2.5 text-xs text-fg-subtle">{unit}</span>}
      </div>
    </div>
  );
}
