import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { formatMhz, formatRelative } from "../format";
import type { Receiver } from "../shell/useReceiver";
import {
  Alert,
  BanIcon,
  Badge,
  Button,
  Kbd,
  NumberField,
  PauseIcon,
  PlainButton,
  PlayIcon,
  SegmentedControl,
  Select,
  Section,
  StopIcon,
  cn,
} from "../ui";
import { VFO_MODES, type VfoMode } from "../vfo/api";
import {
  RANGE_STEPS_HZ,
  bookmarkChannels,
  isRunning,
  planChannels,
  rangeCount,
  sourceProblem,
  startBody,
  statusLabel,
  stoppedText,
  type ResumeMode,
  type ScanHit,
  type ScanSettings,
  type ScanSource,
  type ScannerState,
} from "./model";
import { loadPrefs, savePrefs, type ScannerPrefs, type SourceKind } from "./prefs";

const SOURCES: readonly { value: SourceKind; label: string }[] = [
  { value: "range", label: "Range" },
  { value: "bookmarks", label: "Bookmarks" },
  { value: "plan", label: "Channel plan" },
];

const RESUME_OPTIONS = [
  { value: "hang", label: "After the hang time" },
  { value: "timeout", label: "After a timeout" },
  { value: "hold", label: "Hold until I resume" },
];

const stepLabel = (hz: number) => (hz >= 1000 ? `${hz / 1000} kHz` : `${hz} Hz`);

/**
 * The Scanner tab: pick a range, a bookmark group or a channel plan, start and stop, watch the
 * channel it holds on, skip or lock out, and click a hit to tune the VFO there. Operators and
 * admins control it; everybody else sees the state.
 */
export function ScannerPanel({ r }: { r: Receiver }) {
  const perms = usePermissions();
  const canControl = perms.canTune;
  const scanner: ScannerState | null = r.tuningSnap.state?.scanner ?? null;
  const controls = useSyncExternalStore(r.scan.subscribe, r.scan.getSnapshot);
  const [prefs, setPrefs] = useState<ScannerPrefs>(loadPrefs);
  const update = (patch: Partial<ScannerPrefs>) => setPrefs((p) => ({ ...p, ...patch }));
  const updateSettings = (patch: Partial<ScanSettings>) =>
    setPrefs((p) => ({ ...p, settings: { ...p.settings, ...patch } }));
  useEffect(() => savePrefs(prefs), [prefs]);

  const running = isRunning(scanner);
  const bookmarks = r.bookmarkSnap.bookmarks;
  const groups = r.bookmarkSnap.groups;
  const plans = r.channelSnap.plans;
  const plan = plans.find((p) => p.id === prefs.planId) ?? plans[0] ?? null;

  const source: ScanSource = useMemo(() => {
    if (prefs.source === "range") {
      return {
        kind: "range",
        startHz: Math.round(prefs.startMhz * 1e6),
        stopHz: Math.round(prefs.stopMhz * 1e6),
        stepHz: prefs.stepHz,
        mode: prefs.mode,
      };
    }
    if (prefs.source === "bookmarks") {
      const group = groups.find((g) => g.id === prefs.bookmarkGroup);
      const name =
        prefs.bookmarkGroup === "flagged"
          ? "Bookmarks marked for scanning"
          : prefs.bookmarkGroup === "all"
            ? "All bookmarks"
            : (group?.name ?? "Bookmarks");
      const id =
        prefs.bookmarkGroup === "flagged" || prefs.bookmarkGroup === "all"
          ? prefs.bookmarkGroup
          : (group?.id ?? "all");
      return { kind: "list", name, channels: bookmarkChannels(bookmarks, id) };
    }
    return {
      kind: "list",
      name: plan?.name ?? "Channel plan",
      channels: plan ? planChannels(plan) : [],
    };
  }, [prefs, bookmarks, groups, plan]);
  const problem = sourceProblem(source);
  const count =
    source.kind === "range"
      ? rangeCount(source.startHz, source.stopHz, source.stepHz)
      : source.channels.length;

  const start = () => {
    if (problem !== null) return;
    void r.scan.start(
      startBody(source, prefs.settings, r.selected?.id ?? null, scanner?.lockouts ?? []),
    );
  };
  const noVfo = r.selected === null && !running;
  const hint = canControl ? null : "Operators and admins control the scanner.";

  const window = r.tuningSnap.state?.window;
  return (
    <div className="flex flex-col">
      <Section title="Scanner">
        <div className="flex flex-col gap-3">
          <Status scanner={scanner} />
          {scanner?.state === "idle" && stoppedText(scanner.stopped) && (
            <Alert tone="warn">{stoppedText(scanner.stopped)}</Alert>
          )}
          <div className="flex flex-wrap items-center gap-2">
            {running ? (
              <Button
                variant="danger"
                icon={<StopIcon size={14} />}
                disabled={!canControl || controls.busy}
                onClick={() => void r.scan.stop()}
              >
                Stop <Kbd>G</Kbd>
              </Button>
            ) : (
              <Button
                variant="primary"
                icon={<PlayIcon size={14} />}
                disabled={!canControl || controls.busy || problem !== null || noVfo}
                onClick={start}
              >
                Start <Kbd>G</Kbd>
              </Button>
            )}
            <Button
              icon={<PauseIcon size={14} />}
              disabled={!canControl || !running || controls.busy}
              onClick={() => void (scanner?.state === "paused" ? r.scan.resume() : r.scan.pause())}
            >
              {scanner?.state === "paused" ? "Resume" : "Pause"} <Kbd>P</Kbd>
            </Button>
            <Button
              disabled={!canControl || !running || controls.busy}
              onClick={() => void r.scan.skip()}
            >
              Skip <Kbd>K</Kbd>
            </Button>
            <Button
              icon={<BanIcon size={14} />}
              disabled={!canControl || !scanner?.current || controls.busy}
              onClick={() => void r.scan.lockout()}
              title="Leave this frequency out of the scan"
            >
              Lock out <Kbd>L</Kbd>
            </Button>
          </div>
          {hint && <p className="text-xs text-fg-muted">{hint}</p>}
          {noVfo && canControl && (
            <p className="text-xs text-fg-muted">
              The scanner listens with the active VFO. Create one first.
            </p>
          )}
          {running && scanner?.restricted && (
            <Alert tone="warn">
              The tuning is locked, so the scan stays inside the visible span. Release the lock to
              scan the whole list.
            </Alert>
          )}
        </div>
      </Section>

      <Section title="What to scan" defaultOpen={!running}>
        <div className="flex flex-col gap-3">
          <SegmentedControl
            aria-label="Scan source"
            options={SOURCES}
            value={prefs.source}
            onChange={(v) => update({ source: v })}
            size="sm"
          />
          {prefs.source === "range" && (
            <div className="grid grid-cols-2 gap-2">
              <NumberField
                label="Start (MHz)"
                value={prefs.startMhz}
                decimals={4}
                min={0.001}
                max={10000}
                onCommit={(v) => update({ startMhz: v })}
              />
              <NumberField
                label="Stop (MHz)"
                value={prefs.stopMhz}
                decimals={4}
                min={0.001}
                max={10000}
                onCommit={(v) => update({ stopMhz: v })}
              />
              <Select
                label="Step"
                value={String(prefs.stepHz)}
                options={RANGE_STEPS_HZ.map((hz) => ({ value: String(hz), label: stepLabel(hz) }))}
                onChange={(v) => update({ stepHz: Number(v) })}
              />
              <Select
                label="Mode"
                value={prefs.mode ?? "auto"}
                options={[
                  { value: "auto", label: "By band" },
                  ...VFO_MODES.map((m) => ({ value: m, label: m.toUpperCase() })),
                ]}
                onChange={(v) => update({ mode: v === "auto" ? null : (v as VfoMode) })}
              />
              {window && (
                <Button
                  size="sm"
                  className="col-span-2"
                  onClick={() =>
                    update({
                      startMhz: Math.ceil(window.startHz / 1e4) / 100,
                      stopMhz: Math.floor(window.endHz / 1e4) / 100,
                    })
                  }
                  title="Scan what the spectrum shows: no retuning, the fastest way"
                >
                  Use the visible span
                </Button>
              )}
            </div>
          )}
          {prefs.source === "bookmarks" && (
            <Select
              label="Bookmarks"
              value={prefs.bookmarkGroup}
              options={[
                { value: "flagged", label: "Marked for scanning" },
                { value: "all", label: "All bookmarks" },
                ...groups.map((g) => ({ value: g.id, label: g.name })),
              ]}
              onChange={(v) => update({ bookmarkGroup: v })}
            />
          )}
          {prefs.source === "plan" && (
            <Select
              label="Channel plan"
              value={plan?.id ?? ""}
              options={plans.map((p) => ({ value: p.id, label: p.name }))}
              onChange={(v) => update({ planId: v })}
            />
          )}
          <p className={cn("text-xs", problem ? "text-danger" : "text-fg-muted")}>
            {problem ??
              `${count.toLocaleString("en")} channels. Channels in the visible span are watched without retuning.`}
          </p>
        </div>
      </Section>

      <Section title="Settings" defaultOpen={false}>
        <div className="grid grid-cols-2 gap-2">
          <NumberField
            label="Threshold (dB over noise)"
            value={prefs.settings.thresholdDb}
            min={1}
            max={60}
            decimals={0}
            onCommit={(v) => updateSettings({ thresholdDb: v, thresholdDbfs: null })}
          />
          <NumberField
            label="Dwell (ms)"
            value={prefs.settings.dwellMs}
            min={20}
            max={5000}
            onCommit={(v) => updateSettings({ dwellMs: v })}
          />
          <Select
            label="Resume"
            className="col-span-2"
            value={prefs.settings.resume}
            options={RESUME_OPTIONS}
            onChange={(v) => updateSettings({ resume: v as ResumeMode })}
          />
          {prefs.settings.resume === "hang" && (
            <NumberField
              label="Hang time (ms)"
              value={prefs.settings.hangMs}
              min={0}
              max={60000}
              onCommit={(v) => updateSettings({ hangMs: v })}
            />
          )}
          {prefs.settings.resume === "timeout" && (
            <NumberField
              label="Timeout (ms)"
              value={prefs.settings.timeoutMs}
              min={500}
              max={600000}
              onCommit={(v) => updateSettings({ timeoutMs: v })}
            />
          )}
          <NumberField
            label="Priority channel (MHz)"
            value={prefs.settings.priorityHz === null ? 0 : prefs.settings.priorityHz / 1e6}
            decimals={4}
            min={0}
            max={10000}
            onCommit={(v) => updateSettings({ priorityHz: v > 0 ? Math.round(v * 1e6) : null })}
          />
          <NumberField
            label="Priority check (s)"
            value={prefs.settings.priorityIntervalMs / 1000}
            min={0.5}
            max={600}
            decimals={1}
            onCommit={(v) => updateSettings({ priorityIntervalMs: Math.round(v * 1000) })}
          />
        </div>
        <p className="mt-2 text-xs text-fg-muted">
          0 MHz switches the priority channel off. Settings apply from the next start.
        </p>
      </Section>

      <Section title={`Locked out (${scanner?.lockouts.length ?? 0})`} defaultOpen={false}>
        <Lockouts scanner={scanner} canControl={canControl} r={r} />
      </Section>

      <Section title="Hits">
        <Hits hits={scanner?.hits ?? []} r={r} />
      </Section>
    </div>
  );
}

function Status({ scanner }: { scanner: ScannerState | null }) {
  const status = statusLabel(scanner);
  const cur = scanner?.current ?? null;
  const w = scanner?.window ?? null;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-2 p-3">
      <div className="flex items-center justify-between gap-2">
        <Badge tone={status.tone} dot pulse={scanner?.state === "scanning"}>
          {status.text}
        </Badge>
        {scanner && scanner.state !== "idle" && scanner.config && (
          <span className="truncate text-xs text-fg-muted">
            {scanner.config.name ?? (scanner.config.kind === "range" ? "Range" : "List")} ·{" "}
            {scanner.config.count.toLocaleString("en")} ch
          </span>
        )}
      </div>
      {cur ? (
        <div className="flex flex-col gap-0.5" aria-live="polite">
          <span data-testid="scan-current" className="font-mono text-xl tracking-tight">
            {formatMhz(cur.freqHz)} MHz
          </span>
          <span className="text-xs text-fg-muted">
            {cur.label ? `${cur.label} · ` : ""}
            {cur.mode.toUpperCase()}
            {cur.level ? ` · +${Math.round(cur.level.marginDb)} dB` : ""}
            {cur.priority ? " · priority" : ""}
          </span>
        </div>
      ) : scanner && scanner.state === "scanning" && w ? (
        <span className="text-xs text-fg-muted">
          {formatMhz(w.lowHz)}–{formatMhz(w.highHz)} MHz · window {w.index} of {w.count} · pass{" "}
          {scanner.pass}
        </span>
      ) : (
        <span className="text-xs text-fg-muted">
          {scanner?.state === "idle" || !scanner
            ? "Not running."
            : scanner.state === "paused"
              ? "Paused; the VFO stays where it is."
              : ""}
        </span>
      )}
    </div>
  );
}

function Lockouts({
  scanner,
  canControl,
  r,
}: {
  scanner: ScannerState | null;
  canControl: boolean;
  r: Receiver;
}) {
  const list = scanner?.lockouts ?? [];
  if (list.length === 0) {
    return (
      <p className="text-xs text-fg-muted">
        Nothing is locked out. Use Lock out while the scanner holds a channel.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <ul className="flex flex-wrap gap-1.5">
        {list.map((hz) => (
          <li key={hz}>
            <Button
              size="sm"
              disabled={!canControl}
              onClick={() => void r.scan.unlock(hz)}
              title="Take it off the list"
            >
              {formatMhz(hz)} ×
            </Button>
          </li>
        ))}
      </ul>
      <Button size="sm" disabled={!canControl} onClick={() => void r.scan.unlock()}>
        Clear all
      </Button>
    </div>
  );
}

function Hits({ hits, r }: { hits: readonly ScanHit[]; r: Receiver }) {
  if (hits.length === 0) {
    return (
      <p className="text-xs text-fg-muted">No hits yet. The scanner lists where it stopped.</p>
    );
  }
  return (
    <ul className="flex flex-col" aria-label="Scanner hits">
      {hits.map((h) => (
        <li key={`${h.tsMs}-${h.freqHz}`}>
          <PlainButton
            className="flex w-full items-baseline justify-between gap-2 rounded-md px-1.5 py-1 text-left hover:bg-surface-2"
            title="Tune the active VFO here"
            onClick={() =>
              r.tuneTo({
                freqHz: h.freqHz,
                ...(h.mode ? { mode: h.mode } : {}),
                ...(h.bandwidthHz ? { bandwidthHz: h.bandwidthHz } : {}),
              })
            }
          >
            <span className="min-w-0">
              <span className="font-mono text-sm">{formatMhz(h.freqHz)}</span>
              <span className="ml-2 truncate text-xs text-fg-muted">
                {h.label ?? h.mode?.toUpperCase() ?? ""}
              </span>
            </span>
            <span className="shrink-0 text-xs text-fg-subtle">
              {h.marginDb !== null ? `+${Math.round(h.marginDb)} dB · ` : ""}
              {formatRelative(h.tsMs)}
            </span>
          </PlainButton>
        </li>
      ))}
    </ul>
  );
}
