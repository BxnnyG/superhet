import { lazy, Suspense, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { describeEvent } from "../decoders/render";
import { formatMhz, formatTime, type DecoderEvent } from "../decoders/events";
import { DecoderEventStore } from "../decoders/store";
import { describeHttpError } from "../http";
import type { PluginInstance } from "../plugins/api";
import { InstanceFacts } from "../plugins/InstanceFacts";
import { InstanceSettings, hasSettings } from "../plugins/InstanceSettings";
import { useFlow, verdict } from "../plugins/liveness";
import { StateBadge } from "../plugins/parts";
import type { PluginStore } from "../plugins/store";
import {
  Alert,
  Badge,
  Button,
  ChevronDownIcon,
  Chip,
  PlainButton,
  RefreshIcon,
  SegmentedControl,
  StopIcon,
  cn,
  toasts,
} from "../ui";
import type { Vfo } from "../vfo/api";
import { useLeaveDigitalOnAnalog } from "./actions";
import { digitalOnVfo, expectText, type DigitalMode } from "./modes";

const AprsMapView = lazy(() => import("./AprsMapView").then((m) => ({ default: m.AprsMapView })));

/** How often the dock asks the host for fresh counters. */
export const DOCK_POLL_MS = 1500;
/** Rows kept in the table. */
const MAX_ROWS = 200;
const OPEN_KEY = "superhet.digital-dock-open";

function readOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) !== "0";
  } catch {
    return true;
  }
}

interface Props {
  plugins: PluginStore;
  device: string;
  vfos: readonly Vfo[];
  selectedId: number | null;
  /** Event store of the output table (tests inject one without a network). */
  createEvents?: (device: string) => DecoderEventStore;
}

/**
 * Output panel of the digital modes, docked below the spectrum: for the selected receiver's
 * digital decoder it shows what the decoder says (a table of pages, packets or readings), whether
 * it gets input, and lets the user change its settings live. Renders nothing for an analog
 * receiver. It also stops a decoder whose receiver was switched back to an analog mode.
 */
export function DigitalDock({ plugins, device, vfos, selectedId, createEvents }: Props) {
  const perms = usePermissions();
  const snap = useSyncExternalStore(plugins.subscribe, plugins.getSnapshot);
  useLeaveDigitalOnAnalog(plugins, device, vfos, snap.instances);
  const vfo = vfos.find((v) => v.id === selectedId) ?? null;
  const here = vfo === null ? [] : digitalOnVfo(snap.instances, device, vfo.id);
  const [pick, setPick] = useState<string | null>(null);
  if (!perms.canUsePlugins || vfo === null || here.length === 0) return null;
  const current = here.find((h) => h.instance.id === pick) ?? here[0];
  if (current === undefined) return null;
  return (
    <DockPanel
      key={current.instance.id}
      vfo={vfo}
      mode={current.mode}
      instance={current.instance}
      siblings={here.map((h) => ({ id: h.instance.id, label: h.mode.label }))}
      onPick={setPick}
      plugins={plugins}
      device={device}
      createEvents={createEvents}
    />
  );
}

interface PanelProps {
  vfo: Vfo;
  mode: DigitalMode;
  instance: PluginInstance;
  siblings: readonly { id: string; label: string }[];
  onPick: (id: string) => void;
  plugins: PluginStore;
  device: string;
  createEvents: Props["createEvents"];
}

function DockPanel({
  vfo,
  mode,
  instance,
  siblings,
  onPick,
  plugins,
  device,
  createEvents,
}: PanelProps) {
  const [open, setOpenState] = useState(readOpen);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const flow = useFlow(instance);
  const state = verdict(instance, flow);

  const setOpen = (next: boolean) => {
    setOpenState(next);
    try {
      localStorage.setItem(OPEN_KEY, next ? "1" : "0");
    } catch {
      // Private mode: the panel opens again next time.
    }
  };

  // Fresh counters while the panel is on screen.
  useEffect(() => {
    const t = setInterval(() => {
      if (!document.hidden) void plugins.refreshInstances();
    }, DOCK_POLL_MS);
    return () => clearInterval(t);
  }, [plugins]);

  const act = async (what: "stop" | "restart") => {
    setBusy(true);
    try {
      if (what === "stop") await plugins.stopInstance(instance.id);
      else await plugins.restartInstance(instance.id);
    } catch (e) {
      toasts.push({
        tone: "warn",
        title: `Could not ${what} ${mode.label}`,
        body: describeHttpError(e),
        key: `dock-${what}-${instance.id}`,
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <section
      aria-label={`${mode.label} decoder output`}
      data-testid="digital-dock"
      className="flex max-h-[46%] min-h-0 shrink-0 flex-col border-t border-border bg-surface-1"
    >
      <header className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-2">
        <PlainButton
          aria-expanded={open}
          aria-label={open ? "Collapse decoder output" : "Expand decoder output"}
          onClick={() => setOpen(!open)}
          className="focus-ring inline-flex items-center gap-1.5 rounded-md text-sm font-semibold text-fg"
        >
          <ChevronDownIcon
            size={14}
            className={cn("transition-transform duration-150", !open && "-rotate-90")}
          />
          {mode.label}
        </PlainButton>
        <span className="num text-xs text-fg-muted">
          VFO {vfo.id} · {formatMhz(vfo.freqHz)} MHz
        </span>
        <StateBadge state={instance.state} />
        {siblings.length > 1 && (
          <div role="group" aria-label="Decoders on this receiver" className="flex gap-1">
            {siblings.map((s) => (
              <Chip key={s.id} selected={s.id === instance.id} onClick={() => onPick(s.id)}>
                {s.label}
              </Chip>
            ))}
          </div>
        )}
        <span className="ml-auto flex items-center gap-1.5">
          {hasSettings(instance.pluginId) && (
            <Button
              size="sm"
              variant="secondary"
              aria-expanded={settingsOpen}
              onClick={() => {
                setOpen(true);
                setSettingsOpen(!settingsOpen);
              }}
            >
              Settings
            </Button>
          )}
          {instance.state === "error" && (
            <Button
              size="sm"
              icon={<RefreshIcon size={12} />}
              disabled={busy}
              onClick={() => void act("restart")}
            >
              Restart
            </Button>
          )}
          <Button
            size="sm"
            variant="secondary"
            icon={<StopIcon size={12} />}
            disabled={busy}
            aria-label={`Stop ${mode.label}`}
            onClick={() => void act("stop")}
          >
            Stop
          </Button>
        </span>
      </header>
      <div className="px-3 pb-2">
        <InstanceFacts instance={instance} flow={flow} noun={mode.noun} />
      </div>
      {open && (
        <div className="flex min-h-0 flex-1 flex-col">
          {settingsOpen && (
            <div className="max-h-48 overflow-y-auto border-t border-border px-3 py-3">
              <InstanceSettings instance={instance} store={plugins} />
            </div>
          )}
          <Output
            mode={mode}
            instance={instance}
            device={device}
            createEvents={createEvents}
            verdictText={state.text}
            verdictTone={state.tone}
            vfo={vfo}
          />
        </div>
      )}
    </section>
  );
}

const COLUMNS: Record<DigitalMode["id"], readonly [string, string, string]> = {
  pocsag: ["Address", "Type", "Message"],
  aprs: ["Station", "Type · path", "Position · text"],
  ism: ["Sensor", "Channel", "Readings"],
};

function Output({
  mode,
  instance,
  device,
  createEvents,
  verdictText,
  verdictTone,
  vfo,
}: {
  mode: DigitalMode;
  instance: PluginInstance;
  device: string;
  createEvents: Props["createEvents"];
  verdictText: string;
  verdictTone: "success" | "warn" | "danger" | "neutral" | "accent" | "info";
  vfo: Vfo;
}) {
  // A private event store for the panel: history of this plugin plus the live feed.
  const store = useMemo(
    () => createEvents?.(device) ?? new DecoderEventStore({ device }),
    [device, createEvents],
  );
  useEffect(() => {
    store.start();
    store.setFilter({ kinds: [], source: mode.pluginId, q: "" });
    return () => store.stop();
  }, [store, mode.pluginId]);
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const rows = snap.events.filter((e) => e.meta.instance === instance.id).slice(0, MAX_ROWS);
  const [h1, h2, h3] = COLUMNS[mode.id];
  const [view, setView] = useState<"list" | "map">("list");
  const hasMap = mode.id === "aprs";
  const mapEvents = useMemo(
    () => (hasMap ? snap.events.filter((e) => e.meta.instance === instance.id) : []),
    [hasMap, snap.events, instance.id],
  );
  const problem = instance.state === "running" && verdictTone === "success" ? null : verdictText;

  return (
    <div className="flex min-h-0 flex-1 flex-col border-t border-border">
      {problem !== null && (
        <Alert tone={verdictTone === "danger" ? "danger" : "warn"}>
          <p>{problem}</p>
          {instance.lastError !== null && instance.state !== "running" && (
            <p className="mt-1 break-words text-xs">{instance.lastError}</p>
          )}
        </Alert>
      )}
      {hasMap && rows.length > 0 && (
        <div className="flex items-center gap-2 px-3 py-1.5">
          <SegmentedControl
            size="sm"
            aria-label="APRS view"
            value={view}
            onChange={setView}
            options={[
              { value: "list", label: "List" },
              { value: "map", label: "Map" },
            ]}
          />
        </div>
      )}
      {hasMap && view === "map" && rows.length > 0 ? (
        <Suspense
          fallback={
            <p role="status" className="px-4 py-3 text-sm text-fg-muted">
              Loading the map…
            </p>
          }
        >
          <AprsMapView events={mapEvents} />
        </Suspense>
      ) : rows.length === 0 ? (
        <div className="px-4 py-5 text-sm text-fg-muted" data-testid="dock-empty">
          <p className="font-medium text-fg">
            {instance.state === "running" ? "Listening…" : "Waiting for the decoder"}
          </p>
          <p className="mt-1 max-w-prose">{expectText(mode, vfo.freqHz)}</p>
          {!snap.loaded && <p className="mt-1 text-xs text-fg-subtle">Loading history…</p>}
        </div>
      ) : (
        <div
          className="min-h-0 flex-1 overflow-y-auto"
          role="table"
          aria-label={`${mode.label} output`}
        >
          <div
            role="row"
            className="sticky top-0 z-10 grid grid-cols-[4.5rem_minmax(7rem,12rem)_minmax(9rem,16rem)_minmax(0,1fr)] gap-x-3 border-b border-border bg-surface-1 px-3 py-1 text-[11px] font-semibold tracking-wider text-fg-subtle uppercase max-phone:grid-cols-[3.5rem_minmax(0,1fr)]"
          >
            <span role="columnheader">Time</span>
            <span role="columnheader">{h1}</span>
            <span role="columnheader" className="max-phone:hidden">
              {h2}
            </span>
            <span role="columnheader" className="max-phone:hidden">
              {h3}
            </span>
          </div>
          {rows.map((e) => (
            <Row key={e.id} event={e} />
          ))}
        </div>
      )}
    </div>
  );
}

function Row({ event }: { event: DecoderEvent }) {
  const v = describeEvent(event);
  return (
    <div
      role="row"
      className="grid grid-cols-[4.5rem_minmax(7rem,12rem)_minmax(9rem,16rem)_minmax(0,1fr)] items-baseline gap-x-3 border-b border-border/60 px-3 py-1 text-sm max-phone:grid-cols-[3.5rem_minmax(0,1fr)]"
    >
      <span
        role="cell"
        className="num text-xs text-fg-subtle"
        title={new Date(event.tsMs).toLocaleString()}
      >
        {formatTime(event.tsMs)}
      </span>
      <span
        role="cell"
        className={cn("min-w-0 truncate font-medium text-fg", v.mono && "font-mono")}
        title={v.headline}
      >
        {v.headline}
        <span className="hidden max-phone:block max-phone:font-normal max-phone:text-fg-muted max-phone:break-words">
          {v.detail}
        </span>
      </span>
      <span role="cell" className="flex min-w-0 flex-wrap gap-1 max-phone:hidden">
        {v.tags.map((t) => (
          <Badge key={t} tone="neutral">
            {t}
          </Badge>
        ))}
      </span>
      <span role="cell" className="min-w-0 break-words text-fg-muted max-phone:hidden">
        {v.detail}
        {v.link !== null && (
          <>
            {" "}
            <a
              href={v.link.href}
              target="_blank"
              rel="noopener noreferrer"
              className="text-accent underline-offset-2 hover:underline"
            >
              {v.link.label}
            </a>
          </>
        )}
      </span>
    </div>
  );
}
