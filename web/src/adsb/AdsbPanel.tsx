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
  PlaneIcon,
  PlayIcon,
  StopIcon,
  TextField,
} from "../ui";
import { ADSB_PLUGIN_ID, type AdsbStatus } from "./model";
import { loadReceiverText, parseReceiver, saveReceiverText, type ReceiverText } from "./receiver";

/** The ADS-B tab: start / stop, the headline numbers, and the button for the map and table. */
export function AdsbPanel({ r }: { r: Receiver }) {
  const perms = usePermissions();
  const snap = useSyncExternalStore(r.adsb.subscribe, r.adsb.getSnapshot);
  const pluginSnap = useSyncExternalStore(r.plugins.subscribe, r.plugins.getSnapshot);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [position, setPosition] = useState<ReceiverText>(loadReceiverText);

  const status = snap.reply?.status ?? null;
  const running = snap.instanceId !== null;
  const installed =
    !perms.canUsePlugins ||
    !pluginSnap.loaded ||
    pluginSnap.error !== null ||
    pluginSnap.plugins.some((p) => p.id === ADSB_PLUGIN_ID);
  const parsed = parseReceiver(position);

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

  const start = () =>
    act(async () => {
      if (parsed.error !== null) return;
      saveReceiverText(position);
      const instance = await r.plugins.startInstance({
        pluginId: ADSB_PLUGIN_ID,
        device: r.device,
        config: parsed.receiver
          ? { lat: parsed.receiver.lat, lon: parsed.receiver.lon }
          : undefined,
      });
      setConfirming(false);
      r.adsb.follow(instance.id);
      r.adsb.openView();
    });

  const stop = () =>
    act(async () => {
      if (snap.instanceId === null) return;
      await r.plugins.stopInstance(snap.instanceId);
      r.adsb.closeView();
      r.adsb.follow(null);
    });

  return (
    <div className="flex flex-col gap-3 p-3" data-testid="adsb-panel">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">ADS-B aircraft</h3>
        {running && snap.reply !== null && <StateBadge state={snap.reply.state} />}
        <span className="flex-1" />
        {running && perms.canUsePlugins && (
          <Button
            size="sm"
            variant="danger"
            icon={<StopIcon />}
            disabled={busy}
            onClick={() => void stop()}
            aria-label="Stop ADS-B and give the device back"
          >
            Stop ADS-B
          </Button>
        )}
      </div>
      {snap.error !== null && <Alert tone="warn">{snap.error}</Alert>}
      {problem !== null && !confirming && <Alert>{problem}</Alert>}
      {status?.error != null && <Alert>{status.error}</Alert>}
      {snap.reply?.state === "error" && (
        <Alert>The ADS-B plugin failed and was stopped; see Admin console, Plugins.</Alert>
      )}

      {!running &&
        (perms.canUsePlugins ? (
          installed ? (
            <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-1 p-3">
              <p className="text-xs text-fg-muted">
                Track aircraft on 1090 MHz (ADS-B / Mode S) and watch them on a live map. It takes
                the whole device while it runs and needs an antenna for 1090 MHz.
              </p>
              <Button variant="primary" onClick={() => setConfirming(true)} icon={<PlayIcon />}>
                Start ADS-B…
              </Button>
            </div>
          ) : (
            <Alert tone="warn">The ADS-B plugin is not installed on this server.</Alert>
          )
        ) : (
          <EmptyState title="ADS-B is not running">
            An operator can start it from this tab.
          </EmptyState>
        ))}

      {running && (
        <>
          <Summary status={status} />
          <Button
            variant="primary"
            icon={<PlaneIcon />}
            onClick={() => r.adsb.openView()}
            aria-label="Open the aircraft map and table"
          >
            Open map and table
          </Button>
        </>
      )}

      {confirming && (
        <ConfirmDialog
          title="Start ADS-B on 1090 MHz?"
          confirmLabel="Start ADS-B"
          busy={busy}
          onConfirm={() => void start()}
          onCancel={() => setConfirming(false)}
        >
          <p>
            ADS-B takes the whole device (1090 MHz, 2.4 MS/s). Receivers outside that window are
            paused, nobody can retune until ADS-B is stopped, and the current tuning comes back when
            you stop it. The map loads its tiles from OpenStreetMap.
          </p>
          <fieldset className="mt-3 grid grid-cols-2 gap-2">
            <legend className="mb-1 text-xs font-medium text-fg-muted">
              Receiver position (optional)
            </legend>
            <TextField
              label="Latitude"
              inputMode="decimal"
              placeholder="51.0"
              value={position.lat}
              onChange={(e) => setPosition({ ...position, lat: e.target.value })}
            />
            <TextField
              label="Longitude"
              inputMode="decimal"
              placeholder="7.0"
              value={position.lon}
              onChange={(e) => setPosition({ ...position, lon: e.target.value })}
            />
          </fieldset>
          <p className="mt-1 text-xs text-fg-muted">
            Marks your station on the map and helps decoding. Everybody who may see decoders sees
            it.
          </p>
          {parsed.error !== null && (
            <Alert tone="warn" className="mt-2">
              {parsed.error}
            </Alert>
          )}
          {problem !== null && <Alert className="mt-2">{problem}</Alert>}
        </ConfirmDialog>
      )}
    </div>
  );
}

function Summary({ status }: { status: AdsbStatus | null }) {
  if (status === null || status.mode === "starting") {
    return (
      <p role="status" className="text-xs text-fg-muted">
        Waiting for the decoder…
      </p>
    );
  }
  return (
    <dl className="grid grid-cols-3 gap-2" data-testid="adsb-summary">
      <Stat label="Aircraft" value={String(status.total)} />
      <Stat label="With position" value={String(status.withPosition)} />
      <Stat
        label="Messages/s"
        value={status.messagesPerS === null ? "–" : String(Math.round(status.messagesPerS))}
      />
      {status.total === 0 && (
        <Badge tone="neutral" className="col-span-3 w-fit" title="Check the antenna and gain">
          No aircraft heard yet
        </Badge>
      )}
    </dl>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-border bg-surface-1 px-2.5 py-2">
      <dt className="text-[11px] text-fg-muted">{label}</dt>
      <dd className="text-lg font-semibold tabular-nums">{value}</dd>
    </div>
  );
}
