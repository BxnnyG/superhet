import { useEffect, useRef } from "react";
import { Button, Select, SpeakerIcon, SpeakerMutedIcon } from "../ui";
import type { AudioController, AudioSnapshot } from "./controller";
import { formatDebug } from "./format";

const DEBUG_REFRESH_MS = 500;
export const TARGET_CHOICES_MS = [50, 100, 150, 250] as const;

interface Props {
  audio: AudioController;
  snapshot: AudioSnapshot;
}

/** Audio on/off (a user gesture is required by the autoplay policy). */
export function AudioButton({ audio, snapshot }: Props) {
  const { enabled, starting, unsupported } = snapshot;
  return (
    <Button
      variant={enabled ? "secondary" : "primary"}
      size="md"
      aria-pressed={enabled}
      aria-label={enabled ? "Audio on" : "Audio off"}
      disabled={starting || unsupported !== null}
      title={unsupported ?? "Switch audio output on or off"}
      icon={enabled ? <SpeakerIcon /> : <SpeakerMutedIcon />}
      onClick={() => void (enabled ? audio.disable() : audio.enable())}
    >
      <span className="max-sm:hidden">
        {enabled ? "Audio on" : starting ? "Starting…" : "Audio off"}
      </span>
    </Button>
  );
}

/** Audio section content: on/off, jitter buffer target and the technical readout. */
export function AudioControls({ audio, snapshot }: Props) {
  const debugRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const timer = window.setInterval(() => {
      const el = debugRef.current;
      if (el) el.textContent = formatDebug(audio.getDebug());
    }, DEBUG_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio]);

  const { unsupported } = snapshot;
  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Audio">
      {unsupported ? (
        <p className="rounded-md bg-warn-soft px-2.5 py-2 text-xs text-warn" role="alert">
          {unsupported}
        </p>
      ) : (
        <Select
          label="Jitter buffer"
          value={String(snapshot.targetMs)}
          onChange={(v) => audio.setTargetMs(Number(v))}
          options={TARGET_CHOICES_MS.map((ms) => ({ value: String(ms), label: `${ms} ms` }))}
        />
      )}
      <pre
        ref={debugRef}
        className="min-w-0 overflow-x-auto font-mono text-[11px] leading-snug whitespace-pre-wrap text-fg-subtle"
        title="Latency = capture to browser (includes server/browser clock skew) + jitter buffer + output device"
      />
    </div>
  );
}
