import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { formatFrequency, parseFrequency } from "../spectrum/freq";
import { cn } from "./cn";
import {
  DEFAULT_MAX_HZ,
  DEFAULT_MIN_HZ,
  DIGIT_COUNT,
  HZ_START,
  KHZ_START,
  WHEEL_STEP_PX,
  clampHz,
  digitsOf,
  leadingZeros,
  placeLabel,
  setDigit,
  stepDigit,
  wheelPixels,
} from "./digits";
import { PencilIcon } from "./icons";
import { IconButton } from "./IconButton";

interface Props {
  /** Frequency in Hz; null shows dashes and disables tuning. */
  valueHz: number | null;
  onChange: (hz: number) => void;
  /** Accessible name of the readout, e.g. "Centre frequency". */
  label: string;
  minHz?: number;
  maxHz?: number;
  className?: string;
  /** Read-only (e.g. the tuning lock is on): shows the value, refuses changes. */
  locked?: boolean;
  /** Why it is locked; shown as tooltip of the readout. */
  lockedReason?: string;
  /** `lg` is the primary readout; `sm` a compact secondary one. */
  size?: "sm" | "lg";
  /** Name of a window event that opens the typed entry (the F shortcut, the palette). */
  editEvent?: string;
}

const DEFAULT_FOCUS = 6;

/**
 * Large monospace frequency readout (MHz.kHz.Hz). Every digit tunes on its own: mouse wheel and
 * ArrowUp/ArrowDown step it (with carry), ArrowLeft/Right move between digits, digit keys
 * overwrite. Enter, F2, double-click or the pencil button switch to typed entry.
 */
export function FrequencyReadout({
  valueHz,
  onChange,
  label,
  minHz = DEFAULT_MIN_HZ,
  maxHz = DEFAULT_MAX_HZ,
  className,
  locked = false,
  lockedReason,
  size = "lg",
  editEvent,
}: Props) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [focusIdx, setFocusIdx] = useState(DEFAULT_FOCUS);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [invalid, setInvalid] = useState(false);
  // Latest value including our own not-yet-confirmed changes (fast wheel spins outrun re-renders).
  const latest = useRef(valueHz);
  const wheelAcc = useRef(0);
  const empty = valueHz === null;
  const disabled = empty || locked;

  useEffect(() => {
    latest.current = valueHz;
  }, [valueHz]);

  const emit = (next: number) => {
    latest.current = next;
    onChange(next);
  };

  const step = (index: number, dir: 1 | -1) => {
    const cur = latest.current;
    if (cur === null || locked) return;
    emit(stepDigit(cur, index, dir, minHz, maxHz));
  };

  // React registers wheel listeners as passive; preventDefault needs a native listener.
  const stepRef = useRef(step);
  useEffect(() => {
    stepRef.current = step;
  });
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const onWheel = (e: WheelEvent) => {
      const target = e.target instanceof Element ? e.target.closest("[data-digit]") : null;
      if (!target) return;
      e.preventDefault();
      const index = Number(target.getAttribute("data-digit"));
      const px = wheelPixels(e);
      if (Math.sign(px) !== Math.sign(wheelAcc.current)) wheelAcc.current = 0;
      wheelAcc.current += px;
      if (Math.abs(wheelAcc.current) < WHEEL_STEP_PX) return;
      wheelAcc.current = 0;
      stepRef.current(index, px < 0 ? 1 : -1);
    };
    root.addEventListener("wheel", onWheel, { passive: false });
    return () => root.removeEventListener("wheel", onWheel);
  }, []);

  const focusDigit = (index: number) => {
    const i = Math.min(DIGIT_COUNT - 1, Math.max(0, index));
    setFocusIdx(i);
    rootRef.current?.querySelector<HTMLElement>(`[data-digit="${i}"]`)?.focus();
  };

  const startEdit = () => {
    if (valueHz === null || locked) return;
    setDraft(formatFrequency(valueHz));
    setInvalid(false);
    setEditing(true);
  };

  const startEditRef = useRef(startEdit);
  useEffect(() => {
    startEditRef.current = startEdit;
  });
  useEffect(() => {
    if (!editEvent) return;
    const on = () => startEditRef.current();
    window.addEventListener(editEvent, on);
    return () => window.removeEventListener(editEvent, on);
  }, [editEvent]);

  const stopEdit = () => {
    setEditing(false);
    setInvalid(false);
    requestAnimationFrame(() => focusDigit(focusIdx));
  };

  const onDigitKey = (e: KeyboardEvent, index: number) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    switch (e.key) {
      case "ArrowUp":
        e.preventDefault();
        step(index, 1);
        return;
      case "ArrowDown":
        e.preventDefault();
        step(index, -1);
        return;
      case "ArrowLeft":
        e.preventDefault();
        focusDigit(index - 1);
        return;
      case "ArrowRight":
        e.preventDefault();
        focusDigit(index + 1);
        return;
      case "Home":
        e.preventDefault();
        focusDigit(0);
        return;
      case "End":
        e.preventDefault();
        focusDigit(DIGIT_COUNT - 1);
        return;
      case "Enter":
      case "F2":
        e.preventDefault();
        startEdit();
        return;
    }
    if (/^[0-9]$/.test(e.key) && latest.current !== null && !locked) {
      e.preventDefault();
      emit(setDigit(latest.current, index, Number(e.key), minHz, maxHz));
      focusDigit(index + 1);
    }
  };

  if (editing) {
    return (
      <div className={cn("flex items-center gap-1.5", className)}>
        <input
          type="text"
          inputMode="decimal"
          aria-label={`${label} (type a value, Enter to apply)`}
          aria-invalid={invalid}
          spellCheck={false}
          autoFocus
          value={draft}
          onFocus={(e) => e.currentTarget.select()}
          onChange={(e) => {
            setDraft(e.target.value);
            setInvalid(false);
          }}
          onBlur={() => setEditing(false)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              stopEdit();
            } else if (e.key === "Enter") {
              const hz = parseFrequency(draft);
              if (hz === null) {
                setInvalid(true);
                return;
              }
              emit(clampHz(hz, minHz, maxHz));
              stopEdit();
            }
          }}
          className={cn(
            "rounded-md border bg-surface-1 px-2 text-right font-mono text-fg num",
            size === "sm" ? "h-8 w-36 text-base" : "h-10 w-48 text-2xl",
            "outline-none focus:ring-2 focus:ring-accent/30",
            invalid ? "border-danger" : "border-accent",
          )}
        />
        <span className="text-xs text-fg-subtle">MHz</span>
      </div>
    );
  }

  const digits = digitsOf(valueHz ?? 0);
  const dim = valueHz === null ? DIGIT_COUNT : leadingZeros(valueHz);

  return (
    <div className={cn("flex items-center gap-1", className)}>
      <div
        ref={rootRef}
        role="group"
        aria-label={label}
        title={locked ? lockedReason : undefined}
        onDoubleClick={startEdit}
        className={cn(
          "flex items-baseline font-mono leading-none font-medium text-fg num select-none",
          size === "sm" ? "text-base" : "text-2xl shell:text-3xl",
        )}
      >
        {digits.map((d, i) => (
          <span key={i} className="flex items-baseline">
            {(i === KHZ_START || i === HZ_START) && (
              <span aria-hidden="true" className="px-px text-fg-subtle">
                .
              </span>
            )}
            <span
              role="spinbutton"
              data-digit={i}
              tabIndex={disabled ? -1 : i === focusIdx ? 0 : -1}
              aria-label={`${placeLabel(i)} digit`}
              aria-valuemin={0}
              aria-valuemax={9}
              aria-valuenow={d}
              aria-disabled={disabled}
              onFocus={() => setFocusIdx(i)}
              onClick={() => !disabled && focusDigit(i)}
              onKeyDown={(e) => onDigitKey(e, i)}
              className={cn(
                "focus-ring inline-block min-w-[0.62em] cursor-ns-resize rounded-sm py-1 text-center",
                "transition-colors duration-150",
                !disabled &&
                  "hover:bg-accent-soft focus-visible:bg-accent-soft focus-visible:text-accent",
                i < dim && "text-fg-subtle/60",
                disabled && "cursor-default text-fg-subtle",
              )}
            >
              {empty ? "–" : d}
            </span>
          </span>
        ))}
      </div>
      <span className="ml-1 text-xs font-medium text-fg-subtle">MHz</span>
      <IconButton
        label="Type a frequency"
        tip="Type a frequency (Enter on a digit)"
        size="sm"
        disabled={disabled}
        onClick={startEdit}
      >
        <PencilIcon size={14} />
      </IconButton>
    </div>
  );
}
