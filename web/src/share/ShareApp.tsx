import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { AudioController } from "../audio/controller";
import { Button, Select, TextField } from "../ui";
import {
  PROBLEM_TEXT,
  fetchInfo,
  fetchState,
  type ShareInfo,
  type ShareProblem,
  type ShareState,
} from "./api";
import { shareApi } from "./paths";
import { ShareSpectrum } from "./ShareSpectrum";

const STATE_POLL_MS = 2000;
const INFO_POLL_MS = 15_000;

const fmtMHz = (hz: number): string => (hz / 1e6).toFixed(4).replace(/0+$/, "").replace(/\.$/, "");

/** Fetches `load` now and every `ms`; the first failure is kept as the problem. */
function usePolled<T>(
  load: (
    signal: AbortSignal,
  ) => Promise<{ ok: true; value: T } | { ok: false; problem: ShareProblem }>,
  ms: number,
): { value: T | null; problem: ShareProblem | null } {
  const [res, setRes] = useState<{ value: T | null; problem: ShareProblem | null }>({
    value: null,
    problem: null,
  });
  useEffect(() => {
    const ctrl = new AbortController();
    const run = () =>
      void load(ctrl.signal).then((r) => {
        if (ctrl.signal.aborted) return;
        setRes((old) =>
          r.ok ? { value: r.value, problem: null } : { ...old, problem: r.problem },
        );
      });
    run();
    const id = setInterval(run, ms);
    return () => {
      ctrl.abort();
      clearInterval(id);
    };
  }, [load, ms]);
  return res;
}

/** Audio of a share link: the socket only opens after the visitor pressed play. */
function Player({
  token,
  info,
  state,
  compact,
}: {
  token: string;
  info: ShareInfo;
  state: ShareState | null;
  compact: boolean;
}) {
  const [controller, setController] = useState<AudioController | null>(null);
  const [source, setSource] = useState("receiver");
  const [own, setOwn] = useState<{ vfo: number | null; freqHz: number | null }>({
    vfo: null,
    freqHz: null,
  });
  const [tuneText, setTuneText] = useState(
    info.tuning.mode === "fixed" ? fmtMHz(info.tuning.freqHz) : "",
  );
  const [problem, setProblem] = useState<string | null>(null);
  const live = useRef(controller);
  useEffect(() => {
    live.current = controller;
  });
  useEffect(() => () => live.current?.stop(), []);

  const snap = useSyncExternalStore(
    controller?.subscribe ?? NOOP_SUBSCRIBE,
    controller?.getSnapshot ?? NO_SNAPSHOT,
  );

  const fixed = info.tuning.mode === "fixed";
  const range = info.tuning.mode === "fixed" ? info.tuning.range : null;
  const followed = state?.vfos[0] ?? null;
  const plugin = state?.pluginStreams.find((s) => s.id === source) ?? null;
  const wantedVfo =
    source !== "receiver" ? (plugin?.audioVfo ?? null) : fixed ? own.vfo : (followed?.id ?? null);

  useEffect(() => {
    controller?.setVfos(wantedVfo === null ? [] : [wantedVfo]);
  }, [controller, wantedVfo]);

  const play = useCallback(async () => {
    if (live.current) return;
    const c = new AudioController({
      device: info.device,
      url: shareApi(token).audio(),
      session: false,
      storage: null,
      onMessage: (msg) => {
        if (typeof msg !== "object" || msg === null || !("type" in msg)) return;
        if (msg.type === "hello") {
          const m = msg as { vfo?: unknown; freq_hz?: unknown };
          setOwn({
            vfo: typeof m.vfo === "number" ? m.vfo : null,
            freqHz: typeof m.freq_hz === "number" ? m.freq_hz : null,
          });
        } else if (msg.type === "tuned") {
          const m = msg as { freq_hz?: unknown };
          if (typeof m.freq_hz === "number") {
            const hz = m.freq_hz;
            setOwn((o) => ({ ...o, freqHz: hz }));
            setTuneText(fmtMHz(hz));
          }
        }
      },
    });
    live.current = c;
    setController(c);
    c.start();
    await c.enable();
  }, [info.device, token]);

  const stop = () => {
    live.current?.stop();
    live.current = null;
    setController(null);
    setOwn({ vfo: null, freqHz: null });
  };

  const tune = () => {
    const mhz = Number(tuneText.replace(",", "."));
    if (!Number.isFinite(mhz) || !range) return;
    const hz = Math.round(mhz * 1e6);
    if (hz < range.minHz || hz > range.maxHz) {
      setProblem(
        `Choose a frequency between ${fmtMHz(range.minHz)} and ${fmtMHz(range.maxHz)} MHz.`,
      );
      return;
    }
    setProblem(null);
    controller?.send({ type: "tune", v: 1, freq_hz: hz });
  };

  const playing = snap.enabled;
  const vfoId = wantedVfo;
  const volume = snap.master;
  const options = [
    { value: "receiver", label: "Receiver" },
    ...(state?.pluginStreams ?? []).map((s) => ({ value: s.id, label: s.pluginId })),
  ];

  return (
    <div className="flex flex-col gap-3" data-testid="share-player">
      <div className="flex flex-wrap items-center gap-3">
        {playing ? (
          <Button onClick={stop}>Stop</Button>
        ) : (
          <Button variant="primary" onClick={() => void play()} disabled={snap.starting}>
            {snap.starting ? "Starting…" : "Play"}
          </Button>
        )}
        <label className="flex items-center gap-2 text-xs text-fg-muted">
          Volume
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            disabled={!controller}
            aria-label="Volume"
            onChange={(e) => controller?.setMasterVolume(Number(e.target.value))}
          />
        </label>
        {options.length > 1 && (
          <Select label="Source" options={options} value={source} onChange={setSource} size="sm" />
        )}
        <span className="text-sm tabular-nums text-fg-muted">
          {source === "receiver" && (fixed ? own.freqHz : (followed?.freqHz ?? null)) !== null
            ? `${fmtMHz((fixed ? own.freqHz : followed?.freqHz) ?? 0)} MHz ${
                (fixed
                  ? info.tuning.mode === "fixed"
                    ? info.tuning.demod
                    : ""
                  : followed?.mode
                )?.toUpperCase() ?? ""
              }`
            : ""}
        </span>
      </div>
      {fixed && range && !compact && (
        <div className="flex items-end gap-2">
          <TextField
            label={`Frequency (MHz, ${fmtMHz(range.minHz)} to ${fmtMHz(range.maxHz)})`}
            inputMode="decimal"
            value={tuneText}
            onChange={(e) => setTuneText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") tune();
            }}
          />
          <Button onClick={tune} disabled={!playing || vfoId === null}>
            Tune
          </Button>
        </div>
      )}
      {(problem ?? snap.error) && (
        <p role="alert" className="text-xs text-danger">
          {problem ?? snap.error}
        </p>
      )}
      {snap.unsupported && <p className="text-xs text-fg-muted">{snap.unsupported}</p>}
    </div>
  );
}

const NOOP_SUBSCRIBE = (): (() => void) => () => undefined;
const NO_SNAPSHOT = () => IDLE;
const IDLE = {
  unsupported: null,
  enabled: false,
  starting: false,
  error: null,
  socket: "connecting",
  targetMs: 0,
  master: 0.8,
  levels: {},
  rds: {},
  tones: {},
} as const;

interface FeedEvent {
  kind: string;
  value: string;
}

/** The newest decoder events of the link, newest first. */
function Events({ token }: { token: string }) {
  const [events, setEvents] = useState<FeedEvent[]>([]);
  useEffect(() => {
    let closed = false;
    let ws: WebSocket | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let attempt = 0;
    const open = () => {
      if (closed) return;
      ws = new WebSocket(shareApi(token).events());
      ws.onopen = () => (attempt = 0);
      ws.onmessage = (ev: MessageEvent) => {
        try {
          const msg: unknown = JSON.parse(String(ev.data));
          const e = (msg as { event?: { kind?: unknown; value?: unknown } }).event;
          if (e && typeof e.kind === "string") {
            const item = { kind: e.kind, value: typeof e.value === "string" ? e.value : "" };
            setEvents((old) => [item, ...old].slice(0, 30));
          }
        } catch {
          // Not an event: ignore.
        }
      };
      ws.onclose = () => {
        if (!closed) timer = setTimeout(open, Math.min(10_000, 500 * 2 ** attempt++));
      };
    };
    open();
    return () => {
      closed = true;
      clearTimeout(timer);
      if (ws) {
        ws.onclose = null;
        ws.close();
      }
    };
  }, [token]);
  return (
    <section aria-label="Decoder events" className="rounded-xl border border-border p-3">
      <h2 className="mb-2 text-sm font-semibold">Decoder events</h2>
      {events.length === 0 ? (
        <p className="text-xs text-fg-muted">Waiting for events…</p>
      ) : (
        <ul className="flex max-h-48 flex-col gap-1 overflow-y-auto text-xs">
          {events.map((e, i) => (
            <li key={i} className="flex gap-2">
              <span className="shrink-0 font-medium text-fg-muted">{e.kind}</span>
              <span className="min-w-0 break-words">{e.value}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Message({ children }: { children: string }) {
  return (
    <main className="flex h-full items-center justify-center bg-surface-0 p-6 text-center text-fg">
      <p role="status" className="max-w-sm text-sm text-fg-muted">
        {children}
      </p>
    </main>
  );
}

/**
 * The public receiver page (`/r/<token>`) and the compact embeddable player (`/embed/<token>`).
 * Both are token-scoped: no login, no cookies; what they show is what the link allows.
 */
export function ShareApp({ token, embed }: { token: string; embed: boolean }) {
  const loadInfo = useCallback((s: AbortSignal) => fetchInfo(token, s), [token]);
  const { value: info, problem } = usePolled(loadInfo, INFO_POLL_MS);
  const loadState = useCallback((s: AbortSignal) => fetchState(token, s), [token]);
  const hasState =
    info?.features.includes("waterfall") ||
    info?.features.includes("audio") ||
    info?.features.includes("plugin_audio");
  const { value: state } = usePolled(hasState ? loadState : NO_STATE, STATE_POLL_MS);
  const [socketStatus, setSocketStatus] = useState("connecting");

  if (!info) return <Message>{problem ? PROBLEM_TEXT[problem] : "Loading…"}</Message>;
  if (problem && problem !== "unavailable" && problem !== "busy") {
    return <Message>{PROBLEM_TEXT[problem]}</Message>;
  }
  const waterfall = info.features.includes("waterfall");
  const audio = info.features.includes("audio") || info.features.includes("plugin_audio");
  const events = info.features.includes("decoder_events");
  const listeners = state?.listeners ?? info.listeners;
  const heading = info.title || "Live receiver";

  return (
    <main
      className={`flex h-full flex-col gap-3 overflow-hidden bg-surface-0 text-fg ${embed ? "p-2" : "p-4"}`}
    >
      <header className="flex shrink-0 flex-wrap items-baseline justify-between gap-x-4">
        <h1 className={embed ? "text-sm font-semibold" : "text-lg font-semibold tracking-tight"}>
          {heading}
        </h1>
        <p className="text-xs text-fg-muted">
          {[info.location, `${listeners} listening`].filter(Boolean).join(" · ")}
        </p>
      </header>
      {!embed && info.description && (
        <p className="max-w-3xl shrink-0 whitespace-pre-line text-sm text-fg-muted">
          {info.description}
        </p>
      )}
      {(problem === "unavailable" || socketStatus === "reconnecting") && (
        <p role="status" className="shrink-0 text-xs text-fg-muted">
          Reconnecting…
        </p>
      )}
      {waterfall && <ShareSpectrum token={token} compact={embed} onStatus={setSocketStatus} />}
      {audio && <Player token={token} info={info} state={state} compact={embed} />}
      {events && !embed && <Events token={token} />}
      {!embed && (
        <footer className="shrink-0 text-center text-[11px] text-fg-muted">
          Powered by Superhet
        </footer>
      )}
    </main>
  );
}

const NO_STATE = () =>
  Promise.resolve({ ok: false as const, problem: "unavailable" as ShareProblem });
