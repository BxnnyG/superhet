import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Select, StopIcon, PlayIcon, Toggle, toasts } from "../ui";
import { type RecordingsApi, recordingsApi } from "./api";
import { formatClock, replayChoices, runningFor } from "./model";
import { useRecordings } from "./useRecordings";

type What = "audio" | "iq-vfo" | "iq-window";

const WHAT_OPTIONS = [
  { value: "audio", label: "Audio" },
  { value: "iq-vfo", label: "IQ of this receiver" },
  { value: "iq-window", label: "IQ of the whole band" },
];

const fail = (title: string, e: unknown) =>
  toasts.push({ tone: "danger", title, body: e instanceof Error ? e.message : String(e) });

interface Props {
  device: string;
  vfoId: number;
  /** Operators and admins with control of the device. */
  canRecord: boolean;
  api?: RecordingsApi;
}

/**
 * Record button and instant replay of the selected receiver. Replay plays the last seconds the
 * server buffers (no recording needed); "Save replay" turns them into a recording.
 */
export function RecordReplay({ device, vfoId, canRecord, api = recordingsApi }: Props) {
  const [what, setWhat] = useState<What>("audio");
  const [skipSilence, setSkipSilence] = useState(false);
  const [busy, setBusy] = useState(false);
  const [bufferSeconds, setBufferSeconds] = useState(60);
  const [playing, setPlaying] = useState<number | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const urlRef = useRef<string | null>(null);
  const running = useRecordings({ dev: device, state: "recording" }, { api });
  const active = runningFor(running.items, vfoId);
  // The IQ of the whole band belongs to no VFO: show it as running for any receiver.
  const windowIq = running.items.find(
    (r) => r.state === "recording" && r.kind === "iq" && r.vfo === null,
  );
  const current = active ?? windowIq ?? null;

  useEffect(() => {
    let alive = true;
    void api
      .config()
      .then((c) => alive && setBufferSeconds(c.replaySeconds))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [api]);

  const stopReplay = useCallback(() => {
    audioRef.current?.pause();
    audioRef.current = null;
    if (urlRef.current !== null) URL.revokeObjectURL(urlRef.current);
    urlRef.current = null;
    setPlaying(null);
  }, []);
  useEffect(() => stopReplay, [stopReplay]);

  const play = async (seconds: number) => {
    stopReplay();
    setPlaying(seconds);
    try {
      const blob = await api.replay(device, vfoId, seconds);
      const url = URL.createObjectURL(blob);
      urlRef.current = url;
      const el = new Audio(url);
      audioRef.current = el;
      el.onended = stopReplay;
      await el.play();
    } catch (e) {
      stopReplay();
      fail("Replay failed", e);
    }
  };

  const start = async () => {
    setBusy(true);
    try {
      if (what === "audio") {
        await api.start(device, { kind: "audio", vfo: vfoId, gated: skipSilence });
      } else if (what === "iq-vfo") {
        await api.start(device, { kind: "iq", source: "vfo", vfo: vfoId });
      } else {
        await api.start(device, { kind: "iq", source: "window" });
      }
      running.reload();
    } catch (e) {
      fail("Cannot start the recording", e);
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    if (current === null) return;
    setBusy(true);
    try {
      await api.stop(current.id);
      running.reload();
    } catch (e) {
      fail("Cannot stop the recording", e);
    } finally {
      setBusy(false);
    }
  };

  const save = async (seconds: number) => {
    setBusy(true);
    try {
      const rec = await api.saveReplay(device, vfoId, seconds);
      toasts.push({ tone: "success", title: "Replay saved", body: rec.title });
    } catch (e) {
      fail("Cannot save the replay", e);
    } finally {
      setBusy(false);
    }
  };

  const choices = replayChoices(bufferSeconds);
  return (
    <div className="flex flex-col gap-3">
      {canRecord && (
        <div className="flex flex-col gap-2">
          {current === null ? (
            <>
              <Select
                label="Record"
                value={what}
                options={WHAT_OPTIONS}
                onChange={(v) => setWhat(v as What)}
              />
              {what === "audio" && (
                <Toggle
                  checked={skipSilence}
                  onChange={setSkipSilence}
                  label="Skip silence (squelch closed)"
                />
              )}
              <Button
                variant="primary"
                icon={<span aria-hidden="true" className="size-2.5 rounded-full bg-current" />}
                className="self-start"
                disabled={busy}
                onClick={() => void start()}
              >
                Record
              </Button>
            </>
          ) : (
            <Button
              variant="danger"
              icon={<StopIcon size={14} />}
              className="self-start"
              disabled={busy}
              onClick={() => void stop()}
            >
              Stop recording · {formatClock(current.durationMs)}
            </Button>
          )}
        </div>
      )}
      <div className="flex flex-col gap-1.5">
        <span className="text-xs font-medium text-fg-muted">Instant replay</span>
        <div className="flex flex-wrap gap-2">
          {choices.map((s) => (
            <Button
              key={s}
              variant="secondary"
              size="sm"
              icon={<PlayIcon size={12} />}
              aria-label={`Replay last ${s} seconds`}
              aria-pressed={playing === s}
              onClick={() => (playing === s ? stopReplay() : void play(s))}
            >
              {s} s
            </Button>
          ))}
          {playing !== null && (
            <Button variant="ghost" size="sm" icon={<StopIcon size={12} />} onClick={stopReplay}>
              Stop
            </Button>
          )}
        </div>
        {canRecord && choices.length > 0 && (
          <Button
            variant="secondary"
            size="sm"
            className="self-start"
            disabled={busy}
            onClick={() => void save(choices[Math.min(1, choices.length - 1)] ?? 30)}
          >
            Save last {choices[Math.min(1, choices.length - 1)]} s
          </Button>
        )}
      </div>
    </div>
  );
}
