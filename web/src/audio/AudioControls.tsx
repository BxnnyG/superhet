import { useEffect, useRef } from "react";
import type { AudioController, AudioSnapshot } from "./controller";
import { formatDebug } from "./format";

const DEBUG_REFRESH_MS = 500;
export const TARGET_CHOICES_MS = [50, 100, 150, 250] as const;

interface Props {
  audio: AudioController;
  snapshot: AudioSnapshot;
}

/** "Audio on" button (user gesture for the autoplay policy), jitter target and debug readout. */
export function AudioControls({ audio, snapshot }: Props) {
  const debugRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const timer = window.setInterval(() => {
      const el = debugRef.current;
      if (el) el.textContent = formatDebug(audio.getDebug());
    }, DEBUG_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio]);

  const { enabled, starting, unsupported, error } = snapshot;
  return (
    <div className="flex flex-wrap items-start gap-3 text-sm text-neutral-300" aria-label="Audio">
      <button
        type="button"
        className={`rounded border px-2 py-0.5 disabled:opacity-40 ${
          enabled ? "border-emerald-500 text-emerald-300" : "border-neutral-600"
        }`}
        disabled={starting || unsupported !== null}
        aria-pressed={enabled}
        onClick={() => void (enabled ? audio.disable() : audio.enable())}
      >
        {enabled ? "Audio off" : starting ? "Starting…" : "Audio on"}
      </button>
      {unsupported && (
        <span className="max-w-xl text-amber-400" role="alert">
          {unsupported}
        </span>
      )}
      {error && (
        <span className="text-red-400" role="alert">
          {error}
        </span>
      )}
      {!unsupported && (
        <label className="flex items-center gap-1">
          Buffer
          <select
            className="rounded border border-neutral-600 bg-neutral-900 px-1 py-0.5"
            value={snapshot.targetMs}
            onChange={(e) => audio.setTargetMs(Number(e.target.value))}
          >
            {TARGET_CHOICES_MS.map((ms) => (
              <option key={ms} value={ms}>
                {ms} ms
              </option>
            ))}
          </select>
        </label>
      )}
      <pre
        ref={debugRef}
        className="min-w-0 flex-1 font-mono text-xs leading-snug text-neutral-500"
        title="Latency = capture to browser (includes server/browser clock skew) + jitter buffer + output device"
      />
    </div>
  );
}
