import { useEffect, useState, useSyncExternalStore } from "react";
import {
  loadReceiverText,
  parseReceiver,
  saveReceiverText,
  type ReceiverText,
} from "../adsb/receiver";
import { usePermissions } from "../auth/context";
import { enterDigital } from "../digital/actions";
import { digitalMode } from "../digital/modes";
import { formatDateTime } from "../format";
import { describeHttpError } from "../http";
import { formatBytes } from "../recordings/model";
import type { Receiver } from "../shell/useReceiver";
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  Dialog,
  DownloadIcon,
  EmptyState,
  IconButton,
  PlainButton,
  Section,
  StarIcon,
  TextField,
  Toggle,
  TrashIcon,
  toasts,
} from "../ui";
import { formatMhz } from "../format";
import { weatherApi, type WeatherApi } from "./api";
import {
  ageText,
  clock,
  compass,
  countdown,
  dayLabel,
  imageUrl,
  primaryHz,
  quality,
  WEATHER_PLUGIN_ID,
  type Capture,
  type PassPrediction,
  type PassesReply,
  type Satellite,
  type WeatherConfig,
} from "./model";

/** Refresh of the countdowns (the pass list itself is fetched when the tab opens). */
const TICK_MS = 15_000;
/** Refresh of the gallery while the tab is open: a decoded pass shows up by itself. */
const GALLERY_POLL_MS = 20_000;

const QUALITY_TONE = {
  poor: "neutral",
  fair: "accent",
  good: "success",
  excellent: "success",
} as const;

function useNow(ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

/**
 * The Weather tab: when Meteor-M satellites pass over the receiver, a Tune button that starts the
 * LRPT decoder on a VFO, the gallery of decoded images and, for admins, the source of the orbit
 * data. The decoder records passes by itself while it runs (ADR-0034).
 */
export function WeatherPanel({ r, api = weatherApi }: { r: Receiver; api?: WeatherApi }) {
  const perms = usePermissions();
  const now = useNow(TICK_MS);
  const [config, setConfig] = useState<WeatherConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const [configTick, setConfigTick] = useState(0);
  // What is typed, and what the pass list was asked for ("Use" applies the typed position).
  const [position, setPosition] = useState<ReceiverText>(loadReceiverText);
  const [applied, setApplied] = useState<ReceiverText>(position);
  const [passes, setPasses] = useState<PassesReply | null>(null);
  const [passesError, setPassesError] = useState<string | null>(null);
  const [passesTick, setPassesTick] = useState(0);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const c = await api.config();
        if (!alive) return;
        setConfig(c);
        setConfigError(null);
      } catch (e) {
        if (alive) setConfigError(describeHttpError(e));
      }
    };
    void load();
    return () => {
      alive = false;
    };
  }, [api, configTick]);

  const parsed = parseReceiver(position);
  const appliedPosition = parseReceiver(applied);
  const elementsStamp = config?.elements.fetchedMs ?? null;
  useEffect(() => {
    if (appliedPosition.error !== null) return;
    let alive = true;
    const load = async () => {
      try {
        const reply = await api.passes({
          dev: r.device,
          ...(appliedPosition.receiver === null
            ? {}
            : { lat: appliedPosition.receiver.lat, lon: appliedPosition.receiver.lon }),
          hours: 24,
          minElevationDeg: 10,
        });
        if (!alive) return;
        setPasses(reply);
        setPassesError(null);
      } catch (e) {
        if (!alive) return;
        setPasses(null);
        setPassesError(describeHttpError(e));
      }
    };
    void load();
    return () => {
      alive = false;
    };
    // `appliedPosition` is derived from `applied`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, r.device, applied.lat, applied.lon, elementsStamp, passesTick]);

  const savePosition = () => {
    if (parsed.error !== null) return;
    saveReceiverText(position);
    setApplied(position);
    setPassesTick((t) => t + 1);
  };
  const loadConfig = () => setConfigTick((t) => t + 1);
  const loadPasses = () => setPassesTick((t) => t + 1);

  return (
    <div className="flex flex-col" aria-label="Weather satellites">
      {configError !== null && (
        <p role="alert" className="px-4 py-3 text-sm text-danger">
          {configError}
        </p>
      )}
      {config !== null && (
        <SatelliteStatus satellites={config.satellites} checked={config.checked} />
      )}
      <Section title="Upcoming passes">
        <PassList
          r={r}
          now={now}
          reply={passes}
          error={passesError}
          loading={passes === null && passesError === null}
          position={position}
          onPosition={setPosition}
          onSavePosition={savePosition}
          positionError={parsed.error}
        />
      </Section>
      <Section title="Images">
        <Gallery r={r} api={api} onChanged={loadConfig} />
      </Section>
      {perms.isAdmin && config !== null && (
        <Section title="Orbit data" defaultOpen={config.elements.satellites.length === 0}>
          <ElementsAdmin
            api={api}
            config={config}
            now={now}
            onChanged={() => {
              loadConfig();
              loadPasses();
            }}
          />
        </Section>
      )}
    </div>
  );
}

// ---- satellites ----

function SatelliteStatus({ satellites, checked }: { satellites: Satellite[]; checked: string }) {
  const live = satellites.filter((s) => s.status === "operational");
  const gone = satellites.filter((s) => s.status !== "operational");
  return (
    <div className="border-b border-border px-4 py-3 text-xs text-fg-muted">
      <p>
        <span className="font-medium text-fg">Receivable now:</span>{" "}
        {live.length === 0 ? "none" : live.map((s) => s.name).join(", ")} (LRPT, 137.9 MHz or 137.1
        MHz).
      </p>
      {gone.length > 0 && (
        <details className="mt-1">
          <summary className="cursor-pointer">
            Not receivable: {gone.map((s) => s.name).join(", ")}
          </summary>
          <ul className="mt-1 flex flex-col gap-1">
            {gone.map((s) => (
              <li key={s.noradId}>
                <span className="font-medium">{s.name}:</span> {s.note}
              </li>
            ))}
          </ul>
        </details>
      )}
      {checked !== "" && (
        <p className="mt-1 text-fg-subtle">Status checked {checked}; satellites can change.</p>
      )}
    </div>
  );
}

// ---- passes ----

function PassList({
  r,
  now,
  reply,
  error,
  loading,
  position,
  onPosition,
  onSavePosition,
  positionError,
}: {
  r: Receiver;
  now: number;
  reply: PassesReply | null;
  error: string | null;
  loading: boolean;
  position: ReceiverText;
  onPosition: (p: ReceiverText) => void;
  onSavePosition: () => void;
  positionError: string | null;
}) {
  const upcoming = (reply?.passes ?? []).filter((p) => p.losMs > now);
  return (
    <div className="flex flex-col gap-3 px-4 pb-4">
      <div className="grid grid-cols-[1fr_1fr_auto] items-end gap-2">
        <TextField
          label="Latitude"
          inputMode="decimal"
          value={position.lat}
          onChange={(e) => onPosition({ ...position, lat: e.target.value })}
        />
        <TextField
          label="Longitude"
          inputMode="decimal"
          value={position.lon}
          onChange={(e) => onPosition({ ...position, lon: e.target.value })}
        />
        <Button size="sm" variant="secondary" onClick={onSavePosition}>
          Use
        </Button>
      </div>
      <p className="text-xs text-fg-subtle">
        Your position stays on this server: it is used to compute the passes and is never sent to a
        third party. Leave it empty to use the position ADS-B already has. It is stored in this
        browser, together with the ADS-B position.
      </p>
      {positionError !== null && <Alert tone="warn">{positionError}</Alert>}
      {error !== null && positionError === null && <Alert tone="warn">{error}</Alert>}
      {error === null && reply !== null && upcoming.length === 0 && !loading && (
        <EmptyState title="No pass in the next 24 hours">
          No Meteor-M satellite gets 10 degrees above the horizon in that time.
        </EmptyState>
      )}
      {reply !== null && reply.passes.some((p) => p.elementsAgeH > 72) && (
        <Alert tone="warn">
          The orbit data is more than 3 days old; pass times may be off by a minute or more.
        </Alert>
      )}
      <ul className="flex flex-col gap-2">
        {upcoming.map((p) => (
          <PassRow key={`${p.noradId}-${p.aosMs}`} r={r} pass={p} now={now} />
        ))}
      </ul>
    </div>
  );
}

function PassRow({ r, pass, now }: { r: Receiver; pass: PassPrediction; now: number }) {
  const perms = usePermissions();
  const day = dayLabel(pass.aosMs, now);
  const q = quality(pass.maxElevationDeg);
  const hz = primaryHz(pass);
  return (
    <li
      className="flex flex-col gap-1 rounded-md border border-border bg-surface-1 px-3 py-2"
      data-testid="pass-row"
    >
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{pass.satellite}</p>
          <p className="text-xs text-fg-muted">
            {day !== null && `${day} · `}
            {clock(pass.aosMs)}–{clock(pass.losMs)} · {countdown(pass, now)}
          </p>
        </div>
        <Badge tone={QUALITY_TONE[q]}>{pass.maxElevationDeg.toFixed(0)}° max</Badge>
      </div>
      <p className="text-xs text-fg-subtle">
        Rises {compass(pass.aosAzimuthDeg)}, highest {compass(pass.tcaAzimuthDeg)} at{" "}
        {clock(pass.tcaMs)}, sets {compass(pass.losAzimuthDeg)}
        {hz !== null && ` · ${formatMhz(hz)} MHz ${pass.format}`}
      </p>
      {perms.canUsePlugins && hz !== null && <TuneButton r={r} hz={hz} />}
    </li>
  );
}

/** Put the selected VFO on the satellite and start the LRPT decoder on it. */
function TuneButton({ r, hz }: { r: Receiver; hz: number }) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState(false);
  const snap = useSyncExternalStore(r.plugins.subscribe, r.plugins.getSnapshot);
  const manifest = snap.plugins.find((p) => p.id === WEATHER_PLUGIN_ID);
  const mode = digitalMode("lrpt");

  const run = async (acknowledged: boolean) => {
    const vfo = r.selected;
    if (vfo === null) {
      // No receiver yet: make one on the frequency; the decoder is started with the next press.
      r.tuneTo({ freqHz: hz, mode: "nfm", bandwidthHz: mode.vfo.bandwidthHz });
      toasts.push({
        tone: "info",
        title: "Receiver added",
        body: "Press Tune again to start the weather decoder on it.",
        key: "weather-tune",
      });
      return;
    }
    setBusy(true);
    try {
      const ctx = {
        store: r.plugins,
        device: r.device,
        vfo: { ...vfo, freqHz: hz },
        updateVfo: (id: number, patch: Parameters<typeof r.vfoCtl.update>[1]) =>
          r.vfoCtl.update(id, { ...patch, freqHz: hz }),
      };
      const result = await enterDigital(ctx, mode, acknowledged);
      if (result === "notice") {
        setNotice(true);
        return;
      }
      setNotice(false);
      toasts.push({
        tone: "success",
        title: result === "started" ? "Weather decoder started" : "Receiver tuned",
        body: `${formatMhz(hz)} MHz. It records the pass by itself; the images appear below.`,
        key: "weather-tune",
        durationMs: 5000,
      });
    } catch (e) {
      setNotice(false);
      toasts.push({
        tone: "warn",
        title: "Could not start the weather decoder",
        body: describeHttpError(e),
        key: "weather-tune",
      });
    } finally {
      setBusy(false);
    }
  };

  if (manifest === undefined && snap.loaded) {
    return <p className="text-xs text-fg-subtle">The weather decoder plugin is not installed.</p>;
  }
  return (
    <>
      <div>
        <Button size="sm" variant="secondary" disabled={busy} onClick={() => void run(false)}>
          Tune to {formatMhz(hz)} MHz
        </Button>
      </div>
      {notice && (
        <ConfirmDialog
          title="Start the weather decoder"
          confirmLabel="Start"
          busy={busy}
          onConfirm={() => void run(true)}
          onCancel={() => setNotice(false)}
        >
          <p className="text-sm text-fg-muted">{manifest?.acknowledge?.text ?? mode.title}</p>
        </ConfirmDialog>
      )}
    </>
  );
}

// ---- gallery ----

function Gallery({ r, api, onChanged }: { r: Receiver; api: WeatherApi; onChanged: () => void }) {
  const perms = usePermissions();
  const canManage = perms.isOperator && !perms.deviceReadOnly;
  const [items, setItems] = useState<Capture[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [open, setOpen] = useState<Capture | null>(null);
  const [toDelete, setToDelete] = useState<Capture | null>(null);

  const [tick, setTick] = useState(0);
  const reload = () => setTick((t) => t + 1);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const page = await api.captures({ dev: r.device, limit: 48 });
        if (!alive) return;
        setItems(page.captures);
        setError(null);
      } catch (e) {
        if (alive) setError(describeHttpError(e));
      } finally {
        if (alive) setLoaded(true);
      }
    };
    void load();
    const t = setInterval(() => void load(), GALLERY_POLL_MS);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [api, r.device, tick]);

  const act = async (title: string, run: () => Promise<unknown>) => {
    try {
      await run();
      reload();
      onChanged();
    } catch (e) {
      toasts.push({ tone: "danger", title, body: describeHttpError(e) });
    }
  };

  return (
    <div className="flex flex-col gap-3 px-4 pb-4">
      {error !== null && (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      )}
      {loaded && items.length === 0 && error === null && (
        <EmptyState title="No images yet">
          Start the LRPT decoder (Tune on a pass above, or Decoders → Digital → LRPT) and leave it
          running. Each pass becomes a set of images about a minute after the satellite has set.
        </EmptyState>
      )}
      <ul className="grid grid-cols-2 gap-2">
        {items.map((c) => {
          const first = c.images[0];
          return (
            <li key={c.id} data-testid="capture">
              <PlainButton
                onClick={() => setOpen(c)}
                className="focus-ring flex w-full flex-col overflow-hidden rounded-md border border-border bg-surface-1 text-left hover:border-border-strong"
              >
                {first !== undefined && (
                  <img
                    src={imageUrl(c.id, first.name, { thumb: true })}
                    alt={`${c.satellite ?? "Weather satellite"} ${formatDateTime(c.startedMs)}`}
                    loading="lazy"
                    className="h-32 w-full bg-surface-2 object-cover"
                  />
                )}
                <span className="flex flex-col gap-0.5 px-2 py-1.5">
                  <span className="truncate text-xs font-medium">
                    {c.satellite ?? "Unknown satellite"}
                  </span>
                  <span className="text-xs text-fg-muted">{formatDateTime(c.startedMs)}</span>
                  <span className="text-xs text-fg-subtle">
                    {c.maxElevationDeg === null
                      ? "elevation unknown"
                      : `${c.maxElevationDeg.toFixed(0)}° max`}
                    {c.keep && " · kept"}
                  </span>
                </span>
              </PlainButton>
            </li>
          );
        })}
      </ul>
      {open !== null && (
        <CaptureDialog
          capture={items.find((c) => c.id === open.id) ?? open}
          canManage={canManage}
          onClose={() => setOpen(null)}
          onKeep={(c) => void act("Cannot change Keep", () => api.keep(c.id, !c.keep))}
          onDelete={(c) => {
            setOpen(null);
            setToDelete(c);
          }}
        />
      )}
      {toDelete !== null && (
        <ConfirmDialog
          title="Delete these images?"
          confirmLabel="Delete"
          confirmVariant="danger"
          onConfirm={() => {
            const c = toDelete;
            setToDelete(null);
            void act("Cannot delete", () => api.remove(c.id));
          }}
          onCancel={() => setToDelete(null)}
        >
          <p className="text-sm text-fg-muted">
            {toDelete.satellite ?? "The pass"} of {formatDateTime(toDelete.startedMs)} and its{" "}
            {toDelete.images.length} images ({formatBytes(toDelete.sizeBytes)}) are removed from the
            server.
          </p>
        </ConfirmDialog>
      )}
    </div>
  );
}

function CaptureDialog({
  capture,
  canManage,
  onClose,
  onKeep,
  onDelete,
}: {
  capture: Capture;
  canManage: boolean;
  onClose: () => void;
  onKeep: (c: Capture) => void;
  onDelete: (c: Capture) => void;
}) {
  const [index, setIndex] = useState(0);
  const image = capture.images[Math.min(index, capture.images.length - 1)];
  return (
    <Dialog
      title={`${capture.satellite ?? "Weather satellite"} · ${formatDateTime(capture.startedMs)}`}
      onClose={onClose}
      size="lg"
    >
      <div className="flex flex-col gap-3">
        <p className="text-xs text-fg-muted">
          {capture.pipeline === "apt" ? "APT" : "LRPT"}
          {capture.frequencyHz !== null && ` · ${formatMhz(capture.frequencyHz)} MHz`}
          {` · ${Math.round(capture.durationS / 60)} min`}
          {capture.maxElevationDeg !== null &&
            ` · max elevation ${capture.maxElevationDeg.toFixed(0)}°`}
          {capture.peakDb !== null && ` · signal ${capture.peakDb.toFixed(1)} dB over noise`}
        </p>
        {image !== undefined && (
          <div className="max-h-[60vh] overflow-auto rounded-md border border-border bg-surface-2">
            <img
              src={imageUrl(capture.id, image.name)}
              alt={image.label}
              className="mx-auto block max-w-full"
            />
          </div>
        )}
        {capture.images.length > 1 && (
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Images of this pass">
            {capture.images.map((im, i) => (
              <PlainButton
                key={im.name}
                aria-pressed={i === index}
                onClick={() => setIndex(i)}
                className={`rounded-md border px-2 py-1 text-xs ${
                  i === index
                    ? "border-accent bg-accent/10 text-fg"
                    : "border-border text-fg-muted hover:bg-surface-2"
                }`}
              >
                {im.label}
              </PlainButton>
            ))}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2">
          {image !== undefined && (
            <a
              href={imageUrl(capture.id, image.name, { download: true })}
              download
              className="focus-ring inline-flex h-8 items-center gap-1.5 rounded-md border border-border-strong bg-surface-1 px-3 text-xs font-medium hover:bg-surface-2"
            >
              <DownloadIcon size={12} />
              Download PNG
            </a>
          )}
          {canManage && (
            <>
              <IconButton
                label={capture.keep ? "Allow automatic cleanup" : "Keep (protect from cleanup)"}
                pressed={capture.keep}
                onClick={() => onKeep(capture)}
              >
                <StarIcon size={14} />
              </IconButton>
              <IconButton label="Delete" onClick={() => onDelete(capture)}>
                <TrashIcon size={14} />
              </IconButton>
            </>
          )}
        </div>
      </div>
    </Dialog>
  );
}

// ---- orbit data (admin) ----

function ElementsAdmin({
  api,
  config,
  now,
  onChanged,
}: {
  api: WeatherApi;
  config: WeatherConfig;
  now: number;
  onChanged: () => void;
}) {
  const el = config.elements;
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const act = async (run: () => Promise<unknown>) => {
    setBusy(true);
    setProblem(null);
    try {
      await run();
      onChanged();
    } catch (e) {
      setProblem(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-3 px-4 pb-4">
      <p className="text-xs text-fg-muted">
        Passes are computed from orbit data (TLE / OMM), which ages: a few days is fine for weather
        satellites.{" "}
        {el.satellites.length === 0
          ? "There is none yet."
          : el.satellites
              .map((s) => `${s.name ?? s.noradId}: ${ageText(s.epochMs, now)} old`)
              .join(", ") + "."}
      </p>
      <Toggle
        label={`Download from ${el.download.name} automatically`}
        checked={el.enabled}
        disabled={busy}
        onChange={(enabled) => void act(() => api.setDownload(enabled))}
      />
      <p className="text-xs text-fg-subtle">
        Off by default. When on, the server fetches one fixed {el.download.name} page (the weather
        group,{" "}
        <a
          className="text-accent underline-offset-2 hover:underline"
          href={el.download.infoUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          {el.download.infoUrl}
        </a>
        ) at most every {Math.round(el.download.minIntervalS / 3600)} hours, as
        &quot;Superhet/version&quot;, without your position or any account data.
      </p>
      {el.error !== null && <Alert tone="warn">Last download failed: {el.error}</Alert>}
      {el.enabled && (
        <div>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || el.waitS > 0}
            onClick={() => void act(() => api.refresh())}
          >
            {el.waitS > 0 ? `Download again in ${Math.ceil(el.waitS / 60)} min` : "Download now"}
          </Button>
        </div>
      )}
      <div className="flex flex-col gap-2">
        <label className="text-xs font-medium text-fg-muted" htmlFor="weather-paste">
          Paste TLE or OMM JSON instead
        </label>
        <textarea
          id="weather-paste"
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={4}
          spellCheck={false}
          className="focus-ring w-full rounded-md border border-border-strong bg-surface-1 p-2 font-mono text-xs"
          placeholder={"METEOR-M2 4\n1 59051U 24039A ...\n2 59051  98.7256 ..."}
        />
        <div>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy || text.trim() === ""}
            onClick={() =>
              void act(async () => {
                await api.paste(text);
                setText("");
              })
            }
          >
            Use pasted data
          </Button>
        </div>
      </div>
      {problem !== null && <Alert tone="danger">{problem}</Alert>}
      {el.fetchedMs !== null && (
        <p className="text-xs text-fg-subtle">
          Stored {formatDateTime(el.fetchedMs)} (
          {el.source === "download" ? el.download.name : "pasted"}
          ).
        </p>
      )}
      <p className="text-xs text-fg-subtle">
        Images use {formatBytes(config.store.usedBytes)} of{" "}
        {config.store.maxBytes === 0 ? "unlimited" : formatBytes(config.store.maxBytes)} (
        {config.store.captures} passes); the oldest are removed first, kept ones stay.
      </p>
    </div>
  );
}
