import { useState, type FormEvent } from "react";
import { Button, IconButton, Popover, PencilIcon, TextField } from "../ui";
import type { VfoMode } from "./api";
import { bandwidthRange, checkBandwidth, parseBandwidth } from "./bandwidth";
import { formatBandwidth } from "./model";

interface Props {
  mode: VfoMode;
  /** Current bandwidth in Hz. */
  value: number;
  /** Device sample rate, which bounds the widest channel (unknown: only the mode limits apply). */
  sampleRateHz?: number | null;
  onApply: (hz: number) => void;
  disabled?: boolean;
}

/** Validation of a typed value: the parsed Hz and an error text (null when fine). */
export function validateBandwidthInput(
  text: string,
  mode: VfoMode,
  sampleRateHz?: number | null,
): { hz: number | null; error: string | null } {
  if (text.trim() === "") return { hz: null, error: null };
  const hz = parseBandwidth(text, mode, sampleRateHz);
  if (hz === null) return { hz: null, error: "Enter a width such as 12.5 kHz or 8500 Hz." };
  const check = checkBandwidth(mode, hz, sampleRateHz);
  return check.ok ? { hz, error: null } : { hz, error: check.message };
}

/** Pencil button next to the bandwidth presets: type any width the mode and device allow. */
export function CustomBandwidth({ mode, value, sampleRateHz, onApply, disabled }: Props) {
  return (
    <Popover
      label="Custom bandwidth"
      align="end"
      panelClassName="w-72"
      trigger={({ open, props }) => (
        <IconButton
          label="Custom bandwidth"
          tip="Type a bandwidth"
          size="sm"
          pressed={open}
          disabled={disabled}
          {...props}
        >
          <PencilIcon size={14} />
        </IconButton>
      )}
    >
      {(close) => (
        <CustomForm
          mode={mode}
          value={value}
          sampleRateHz={sampleRateHz}
          onApply={(hz) => {
            onApply(hz);
            close();
          }}
        />
      )}
    </Popover>
  );
}

function CustomForm({
  mode,
  value,
  sampleRateHz,
  onApply,
}: Pick<Props, "mode" | "value" | "sampleRateHz" | "onApply">) {
  const [text, setText] = useState(String(value / 1000));
  const { hz, error } = validateBandwidthInput(text, mode, sampleRateHz);
  const { min, max } = bandwidthRange(mode, sampleRateHz);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (hz !== null && error === null) onApply(hz);
  };
  return (
    <form onSubmit={submit} className="flex flex-col gap-2">
      <TextField
        label="Bandwidth"
        value={text}
        autoFocus
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setText(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        error={error}
        hint={`${formatBandwidth(min)} to ${formatBandwidth(max)}. A bare number is kHz; add Hz to be exact.`}
      />
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs text-fg-muted" aria-live="polite">
          {hz !== null && error === null ? `= ${formatBandwidth(hz)}` : ""}
        </span>
        <Button type="submit" size="sm" variant="primary" disabled={hz === null || error !== null}>
          Apply
        </Button>
      </div>
    </form>
  );
}
