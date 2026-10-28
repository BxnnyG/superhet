import { useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { describeHttpError } from "../http";
import { StateBadge } from "../plugins/parts";
import type { Receiver } from "../shell/useReceiver";
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  PlayIcon,
  RowButton,
  Select,
  StopIcon,
  Toggle,
  cn,
} from "../ui";
import { mediaUrl } from "./api";
import {
  DAB_BLOCKS,
  DAB_PLUGIN_ID,
  blockFreq,
  signalGrade,
  slideKey,
  type DabFound,
  type DabService,
  type DabStatus,
  type SignalGrade,
} from "./model";

const GRADE_TONE: Record<SignalGrade, "success" | "warn" | "danger" | "neutral"> = {
  good: "success",
  fair: "warn",
  poor: "danger",
  none: "neutral",
};
const GRADE_LABEL: Record<SignalGrade, string> = {
  good: "Good signal",
  fair: "Fair signal",
  poor: "Weak signal",
  none: "No signal",
};

/** The DAB / DAB+ view: ensemble scan, service list, now playing with slideshow, play buttons. */
export function DabPanel({ r }: { r: Receiver }) {
  const perms = usePermissions();
  const snap = useSyncExternalStore(r.dab.subscribe, r.dab.getSnapshot);
  const pluginSnap = useSyncExternalStore(r.plugins.subscribe, r.plugins.getSnapshot);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [block, setBlock] = useState<string>("5C");

  const status = snap.reply?.status ?? null;
  const running = snap.instanceId !== null;
  const installed =
    !perms.canUsePlugins ||
    !pluginSnap.loaded ||
    pluginSnap.error !== null ||
    pluginSnap.plugins.some((p) => p.id === DAB_PLUGIN_ID);

  const act = async (what: () => Promise<unknown>) => {
    setBusy(true);
    setProblem(null);
    try {
      await what();
    } catch (e) {
      setProblem(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  const start = (channel: string) =>
    act(async () => {
      const freqHz = blockFreq(channel);
      if (freqHz === null) return;
      const instance = await r.plugins.startInstance({
        pluginId: DAB_PLUGIN_ID,
        device: r.device,
        freqHz,
      });
      r.dab.request(null);
      r.dab.follow(instance.id);
    });

  const stop = () =>
    act(async () => {
      if (snap.instanceId === null) return;
      await r.plugins.stopInstance(snap.instanceId);
      r.dab.follow(null);
    });

  const tune = (channel: string) => act(() => r.dab.control({ cmd: "tune", channel }));
  const play = (sid: string | null) => act(() => r.dab.control({ cmd: "select", service: sid }));

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="dab-panel">
      <Header
        running={running}
        state={snap.reply?.state ?? null}
        status={status}
        canStop={perms.canUsePlugins}
        busy={busy}
        onStop={() => void stop()}
      />
      {snap.error !== null && <Alert tone="warn">{snap.error}</Alert>}
      {problem !== null && <Alert>{problem}</Alert>}
      {status?.error != null && <Alert>{status.error}</Alert>}
      {snap.reply?.state === "error" && (
        <Alert>The DAB plugin failed and was stopped; see Admin console, Plugins.</Alert>
      )}

      {!running && (
        <Idle
          canStart={perms.canUsePlugins}
          installed={installed}
          block={block}
          setBlock={setBlock}
          onStart={() => r.dab.request({ channel: block, freqHz: blockFreq(block) ?? 0 })}
        />
      )}

      {running && (
        <>
          {status !== null && status.mode === "scanning" && (
            <ScanProgress
              status={status}
              canStop={perms.canUsePlugins}
              onStop={() => void act(() => r.dab.control({ cmd: "scan_stop" }))}
            />
          )}
          {status !== null && status.mode !== "scanning" && (
            <Tuner
              status={status}
              known={snap.known}
              canTune={perms.canUsePlugins}
              busy={busy}
              block={block}
              setBlock={setBlock}
              onTune={(c) => void tune(c)}
              onScan={() => void act(() => r.dab.control({ cmd: "scan" }))}
            />
          )}
          {status !== null && (
            <Services
              status={status}
              canPlay={perms.canUsePlugins}
              onPlay={(sid) => void play(sid)}
              audioReady={r.audioSnap.enabled}
              onEnableAudio={() => void r.audio.enable()}
              vfoId={snap.reply?.audioVfo ?? null}
              r={r}
            />
          )}
          {status !== null && snap.instanceId !== null && (
            <NowPlaying instanceId={snap.instanceId} status={status} reply={snap.reply} />
          )}
        </>
      )}

      {snap.pending !== null && (
        <ConfirmDialog
          title={`Start DAB on ${snap.pending.channel}?`}
          confirmLabel="Start DAB"
          busy={busy}
          onConfirm={() => void start(snap.pending?.channel ?? block)}
          onCancel={() => r.dab.request(null)}
        >
          <p>
            DAB takes the whole device ({(snap.pending.freqHz / 1e6).toFixed(3)} MHz, 2.048 MS/s).
            Receivers outside that window are paused, nobody can retune until DAB is stopped, and
            the current tuning comes back when you stop it.
          </p>
          {problem !== null && <Alert className="mt-2">{problem}</Alert>}
        </ConfirmDialog>
      )}
    </div>
  );
}

function Header({
  running,
  state,
  status,
  canStop,
  busy,
  onStop,
}: {
  running: boolean;
  state: import("../plugins/api").InstanceState | null;
  status: DabStatus | null;
  canStop: boolean;
  busy: boolean;
  onStop: () => void;
}) {
  const grade = signalGrade(status);
  return (
    <div className="flex flex-wrap items-center gap-2">
      <h3 className="text-sm font-semibold">DAB / DAB+</h3>
      {running && state !== null && <StateBadge state={state} />}
      {running && status !== null && status.mode !== "idle" && (
        <Badge tone={GRADE_TONE[grade]} title={`SNR ${status.snr?.toFixed(1) ?? "?"} dB`}>
          {GRADE_LABEL[grade]}
          {status.snr !== null && status.synced ? ` · ${status.snr.toFixed(0)} dB` : ""}
        </Badge>
      )}
      <span className="flex-1" />
      {running && canStop && (
        <Button
          size="sm"
          variant="danger"
          icon={<StopIcon />}
          disabled={busy}
          onClick={onStop}
          aria-label="Stop DAB and give the device back"
        >
          Stop DAB
        </Button>
      )}
    </div>
  );
}

function blockOptions(known: readonly DabFound[]) {
  const labels = new Map(known.map((k) => [k.channel, k.label]));
  return DAB_BLOCKS.map(([name]) => ({
    value: name,
    label: labels.has(name) ? `${name} · ${labels.get(name)}` : name,
  }));
}

function Idle({
  canStart,
  installed,
  block,
  setBlock,
  onStart,
}: {
  canStart: boolean;
  installed: boolean;
  block: string;
  setBlock: (b: string) => void;
  onStart: () => void;
}) {
  if (!canStart) {
    return (
      <EmptyState title="DAB is not running">
        An operator can start it from this tab or from the DAB channel plan.
      </EmptyState>
    );
  }
  if (!installed) {
    return <Alert tone="warn">The DAB plugin is not installed on this server.</Alert>;
  }
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-1 p-3">
      <p className="text-xs text-fg-muted">
        Receive DAB / DAB+ with the device. It takes the whole device while it runs (an ensemble is
        1.5 MHz wide). Pick a block, or start anywhere and run a scan.
      </p>
      <Select
        label="Block"
        value={block}
        onChange={setBlock}
        options={blockOptions([])}
        size="sm"
      />
      <Button variant="primary" onClick={onStart} icon={<PlayIcon />}>
        Start DAB…
      </Button>
    </div>
  );
}

function Tuner({
  status,
  known,
  canTune,
  busy,
  block,
  setBlock,
  onTune,
  onScan,
}: {
  status: DabStatus;
  known: readonly DabFound[];
  canTune: boolean;
  busy: boolean;
  block: string;
  setBlock: (b: string) => void;
  onTune: (channel: string) => void;
  onScan: () => void;
}) {
  const found = status.lastScan?.found ?? [];
  // Scan results of this run first, then what the history knows about other blocks.
  const merged = [...found, ...known.filter((k) => !found.some((f) => f.channel === k.channel))];
  merged.sort((a, b) => (blockFreq(a.channel) ?? 0) - (blockFreq(b.channel) ?? 0));
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="text-xs text-fg-subtle">Block</span>
        <span className="text-sm font-semibold">{status.channel ?? "-"}</span>
        <span className="min-w-0 flex-1 truncate text-sm" data-testid="dab-ensemble">
          {status.ensemble ? status.ensemble.label || status.ensemble.eid : "searching…"}
        </span>
        {status.ensemble && (
          <span className="text-xs text-fg-subtle">
            {status.ensemble.eid} · {status.services.length} services
          </span>
        )}
      </div>
      {canTune && (
        <div className="flex items-end gap-2">
          <Select
            label="Tune to block"
            value={block}
            onChange={setBlock}
            options={blockOptions(merged)}
            size="sm"
            className="flex-1"
          />
          <Button size="sm" disabled={busy} onClick={() => onTune(block)}>
            Tune
          </Button>
          <Button size="sm" disabled={busy} onClick={onScan}>
            Scan all blocks
          </Button>
        </div>
      )}
      {merged.length > 0 && (
        <ul aria-label="Ensembles found" className="flex flex-col gap-1">
          {merged.map((f) => (
            <li key={f.channel}>
              <RowButton
                selected={f.channel === status.channel}
                disabled={!canTune || busy}
                onClick={() => onTune(f.channel)}
                className="w-full items-baseline text-sm disabled:cursor-default"
              >
                <span className="w-9 shrink-0 font-medium">{f.channel}</span>
                <span className="min-w-0 flex-1 truncate">{f.label || f.eid}</span>
                <span className="shrink-0 text-xs text-fg-subtle">{f.services} services</span>
              </RowButton>
            </li>
          ))}
        </ul>
      )}
      {status.lastScan && merged.length === 0 && (
        <p className="text-xs text-fg-muted">The last scan found no ensemble. Check the antenna.</p>
      )}
    </div>
  );
}

function ScanProgress({
  status,
  canStop,
  onStop,
}: {
  status: DabStatus;
  canStop: boolean;
  onStop: () => void;
}) {
  const scan = status.scan;
  if (scan === null) return null;
  const pct = scan.total > 0 ? Math.round((scan.index / scan.total) * 100) : 0;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-1 p-3">
      <div className="flex items-center gap-2 text-sm">
        <span className="font-medium">Scanning {scan.channel ?? ""}</span>
        <span className="text-xs text-fg-subtle">
          {Math.min(scan.index + 1, scan.total)} of {scan.total}
        </span>
        <span className="flex-1" />
        {canStop && (
          <Button size="sm" onClick={onStop}>
            Stop scan
          </Button>
        )}
      </div>
      <div
        role="progressbar"
        aria-label="Scan progress"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        className="h-1.5 overflow-hidden rounded-full bg-surface-3"
      >
        <div className="h-full bg-accent transition-[width]" style={{ width: `${pct}%` }} />
      </div>
      {scan.found.length > 0 && (
        <ul aria-label="Found so far" className="text-sm">
          {scan.found.map((f) => (
            <li key={f.channel}>
              {f.channel} · {f.label || f.eid}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Services({
  status,
  canPlay,
  onPlay,
  audioReady,
  onEnableAudio,
  vfoId,
  r,
}: {
  status: DabStatus;
  canPlay: boolean;
  onPlay: (sid: string | null) => void;
  audioReady: boolean;
  onEnableAudio: () => void;
  vfoId: number | null;
  r: Receiver;
}) {
  const level = vfoId !== null ? r.audioSnap.levels[vfoId] : undefined;
  if (status.services.length === 0) {
    return status.mode === "scanning" ? null : (
      <p className="text-xs text-fg-muted">
        {status.synced
          ? "Reading the service list…"
          : "No ensemble on this block yet. Wait a few seconds or pick another block."}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <h4 className="text-xs font-medium text-fg-muted">Services</h4>
        <span className="flex-1" />
        {!audioReady && (
          <Button size="sm" variant="secondary" onClick={onEnableAudio}>
            Enable audio
          </Button>
        )}
        {level && (
          <Toggle
            label="Listen"
            checked={level.listening}
            onChange={(on) => vfoId !== null && r.audio.setListening(vfoId, on)}
          />
        )}
      </div>
      <ul aria-label="Services" className="flex flex-col">
        {status.services
          .filter((s) => s.audio)
          .map((s) => (
            <ServiceRow
              key={s.sid}
              service={s}
              playing={status.selected === s.sid}
              canPlay={canPlay}
              onPlay={onPlay}
            />
          ))}
      </ul>
    </div>
  );
}

function ServiceRow({
  service,
  playing,
  canPlay,
  onPlay,
}: {
  service: DabService;
  playing: boolean;
  canPlay: boolean;
  onPlay: (sid: string | null) => void;
}) {
  const name = service.label || service.sid;
  return (
    <li
      className={cn("flex items-center gap-2 rounded-md px-2 py-1.5", playing && "bg-accent-soft")}
    >
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{name}</div>
        <div className="truncate text-xs text-fg-subtle">
          {[service.pty, service.codec, service.bitrate !== null ? `${service.bitrate} kbit/s` : ""]
            .filter((x) => x !== "")
            .join(" · ")}
        </div>
      </div>
      {canPlay && (
        <Button
          size="sm"
          variant={playing ? "primary" : "secondary"}
          aria-label={playing ? `Stop ${name}` : `Play ${name}`}
          aria-pressed={playing}
          icon={playing ? <StopIcon /> : <PlayIcon />}
          onClick={() => onPlay(playing ? null : service.sid)}
        />
      )}
      {!canPlay && playing && <Badge tone="success">Playing</Badge>}
    </li>
  );
}

function NowPlaying({
  instanceId,
  status,
  reply,
}: {
  instanceId: string;
  status: DabStatus;
  reply: import("./model").DabReply | null;
}) {
  const service = status.services.find((s) => s.sid === status.selected);
  if (!service) return null;
  const slide = reply?.media.find((m) => m.key === slideKey(service.sid));
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-1 p-3">
      <div className="text-xs text-fg-subtle">Now playing</div>
      <div className="text-sm font-semibold">{service.label || service.sid}</div>
      <p className="text-sm text-fg-muted" data-testid="dab-dls" aria-live="polite">
        {service.dls ?? (status.playing ? "No station text yet." : "Starting the audio…")}
      </p>
      {slide && (
        <img
          src={mediaUrl(instanceId, slide.key, slide.revision)}
          alt={`Slideshow image of ${service.label || service.sid}`}
          className="max-h-60 w-full rounded-md bg-surface-2 object-contain"
        />
      )}
    </div>
  );
}
