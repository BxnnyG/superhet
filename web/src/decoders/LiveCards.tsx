import type { ReactNode } from "react";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import {
  ptyName,
  rdsQualityLevel,
  rdsTooWeakForText,
  type RdsQualityLevel,
  type RdsState,
} from "../audio/rds";
import { formatCtcss, type ToneState } from "../audio/tones";
import { Badge, Button, EmptyState, Select, Toggle, cn } from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import { MODE_LABEL, vfoColor } from "../vfo/model";
import { SQUELCH_OPTIONS, copyDigits } from "../vfo/TonesPanel";
import { formatMhz } from "./events";

type Update = (id: number, patch: VfoPatch) => void;

interface Props {
  vfos: readonly Vfo[];
  selectedId: number | null;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  onUpdate: Update;
}

/** FM broadcast band: where RDS is transmitted. */
export const RDS_BAND_HZ = { min: 87_500_000, max: 108_000_000 } as const;
/** RDS sits at 57 kHz on the baseband; a narrower channel filter cuts it off. */
export const RDS_MIN_BANDWIDTH_HZ = 120_000;

/** Whether an RDS card makes sense: WFM, on the FM broadcast band, wide enough for the subcarrier. */
export const hasRds = (v: Vfo): boolean =>
  v.mode === "wfm" &&
  v.freqHz >= RDS_BAND_HZ.min &&
  v.freqHz <= RDS_BAND_HZ.max &&
  v.bandwidthHz >= RDS_MIN_BANDWIDTH_HZ;

/** True for the modes that have a decoder card. */
export const hasDecoder = (v: Vfo): boolean => hasRds(v) || v.mode === "nfm" || v.mode === "am";

/**
 * Live state of the decoders, one card per VFO that has any: RDS for WFM, CTCSS and DTMF for
 * NFM and AM. The selected VFO comes first and is outlined.
 */
export function LiveCards({ vfos, selectedId, audio, audioSnap, onUpdate }: Props) {
  const cards = vfos
    .filter(hasDecoder)
    .sort((a, b) => Number(b.id === selectedId) - Number(a.id === selectedId) || a.id - b.id);
  if (cards.length === 0) {
    return (
      <div className="px-4 py-3">
        <EmptyState
          title={vfos.length === 0 ? "No receiver selected" : "No decoders for this mode"}
        >
          RDS needs a WFM receiver on the FM broadcast band (87.5 to 108 MHz, bandwidth 120 kHz or
          more); CTCSS and DTMF need NFM or AM. Pagers, APRS and sensors have their own output panel
          below the spectrum (mode bar: Digital). The log below keeps what was heard.
        </EmptyState>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2 px-3 py-3" aria-label="Live decoders" role="group">
      {cards.map((v) =>
        hasRds(v) ? (
          <RdsCard key={v.id} vfo={v} rds={audioSnap.rds[v.id]} selected={v.id === selectedId} />
        ) : (
          <TonesCard
            key={v.id}
            vfo={v}
            tones={audioSnap.tones[v.id]}
            selected={v.id === selectedId}
            audio={audio}
            onUpdate={onUpdate}
          />
        ),
      )}
    </div>
  );
}

function Card({
  vfo,
  selected,
  label,
  status,
  children,
}: {
  vfo: Vfo;
  selected: boolean;
  label: string;
  status?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={`${label} of VFO ${vfo.id}`}
      className={cn(
        "flex flex-col gap-2 rounded-lg border bg-surface-1 p-3 shadow-sm",
        selected ? "border-accent/50 ring-1 ring-accent/20" : "border-border",
      )}
    >
      <header className="flex min-w-0 items-center gap-2">
        <span
          aria-hidden="true"
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: vfoColor(vfo.id) }}
        />
        <span className="text-xs font-semibold tracking-wider text-fg-muted uppercase">
          {label}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-fg-subtle num">
          VFO {vfo.id} · {formatMhz(vfo.freqHz)} MHz · {MODE_LABEL[vfo.mode]}
        </span>
        {status}
      </header>
      {children}
    </section>
  );
}

export function RdsCard({
  vfo,
  rds,
  selected,
}: {
  vfo: Vfo;
  rds: RdsState | undefined;
  selected: boolean;
}) {
  const synced = rds?.synced === true;
  const hasData = rds !== undefined;
  const status = synced ? (
    <Badge tone="success" dot title="Receiving RDS blocks right now">
      RDS sync
    </Badge>
  ) : hasData ? (
    <Badge tone="warn" title="Showing the last decoded data; blocks are no longer received">
      signal lost
    </Badge>
  ) : (
    <Badge tone="neutral" title="No RDS blocks received yet">
      no RDS sync
    </Badge>
  );
  return (
    <Card vfo={vfo} selected={selected} label="RDS" status={status}>
      <div
        data-testid="rds-ps"
        title="Programme service name"
        className={cn(
          "min-h-8 font-mono text-2xl font-semibold tracking-[0.18em] whitespace-pre text-fg",
          (!hasData || rds.ps === null || !synced) && "opacity-60",
        )}
      >
        {rds?.ps ?? "········"}
      </div>
      {hasData ? (
        <>
          <p
            data-testid="rds-rt"
            title={rds.rt ?? undefined}
            className="line-clamp-2 min-h-5 text-sm text-fg-muted"
          >
            {rds.rt ?? <span className="text-fg-subtle">no RadioText yet</span>}
          </p>
          <RdsQuality rds={rds} />
          <div className="flex flex-wrap items-center gap-1.5">
            {rds.pi !== null && (
              <Badge tone="neutral" title="Programme identification">
                <span className="font-mono">PI {rds.pi}</span>
              </Badge>
            )}
            {rds.pty !== null && rds.pty > 0 && (
              <Badge tone="accent" title={`Programme type ${rds.pty}`}>
                {ptyName(rds.pty, vfo.deemphasis)}
              </Badge>
            )}
            {rds.tp === true && (
              <Badge tone="neutral" title="Traffic programme">
                TP
              </Badge>
            )}
            {rds.ta === true && (
              <Badge tone="warn" title="Traffic announcement in progress">
                TA
              </Badge>
            )}
            {rds.ms !== null && (
              <Badge tone="neutral" title={rds.ms ? "Music" : "Speech"}>
                {rds.ms ? "Music" : "Speech"}
              </Badge>
            )}
          </div>
        </>
      ) : (
        <p className="text-xs text-fg-subtle">
          Waiting for RDS. It needs a steady signal and appears about a second after tuning.
        </p>
      )}
    </Card>
  );
}

const QUALITY_LABEL: Record<RdsQualityLevel, string> = {
  none: "No sync",
  good: "Good",
  fair: "Fair",
  poor: "Poor",
};

const QUALITY_BAR: Record<RdsQualityLevel, string> = {
  none: "bg-fg-subtle",
  good: "bg-success",
  fair: "bg-warn",
  poor: "bg-danger",
};

const QUALITY_TEXT: Record<RdsQualityLevel, string> = {
  none: "text-fg-subtle",
  good: "text-success",
  fair: "text-warn",
  poor: "text-danger",
};

/** Reception quality of the RDS subcarrier: bar, grade and block error rate of the last ~10 s. */
function RdsQuality({ rds }: { rds: RdsState }) {
  const level = rdsQualityLevel(rds);
  const quality = rds.quality ?? 0;
  const detail =
    rds.bler === null
      ? "Measuring…"
      : `${rds.bler} % of the RDS blocks failed in the last 10 s (good: up to 10 %, fair: up to 35 %).`;
  return (
    <div className="flex flex-col gap-1" data-testid="rds-quality" data-level={level}>
      <div
        className="flex items-center gap-2"
        title={`${detail} Weak or clipped reception loses station name and RadioText first.`}
      >
        <span className="w-12 shrink-0 text-[11px] text-fg-subtle">Signal</span>
        <div
          role="meter"
          aria-label="RDS reception quality"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={quality}
          aria-valuetext={`${QUALITY_LABEL[level]}${rds.bler === null ? "" : `, ${rds.bler} % block errors`}`}
          className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-3"
        >
          <div
            className={cn(
              "h-full rounded-full transition-[width] duration-300",
              QUALITY_BAR[level],
            )}
            style={{ width: `${quality}%` }}
          />
        </div>
        <span className={cn("shrink-0 text-[11px] font-medium num", QUALITY_TEXT[level])}>
          {QUALITY_LABEL[level]}
          {rds.bler !== null && <span className="text-fg-subtle"> · {rds.bler} % BLER</span>}
        </span>
      </div>
      {rdsTooWeakForText(rds) && (
        <p role="status" className="text-xs text-warn">
          Signal too weak for the station name and RadioText. Try a better antenna position or a
          different gain.
        </p>
      )}
    </div>
  );
}

export function TonesCard({
  vfo,
  tones,
  selected,
  audio,
  onUpdate,
}: {
  vfo: Vfo;
  tones: ToneState | undefined;
  selected: boolean;
  audio: Pick<AudioController, "clearDigits">;
  onUpdate: Update;
}) {
  const enabled = vfo.tones === true;
  const squelchHz = vfo.toneSquelchHz ?? null;
  const detected = tones?.ctcssHz ?? null;
  const digits = tones?.digits ?? "";
  return (
    <Card
      vfo={vfo}
      selected={selected}
      label="Tones"
      status={
        <Toggle
          label="Detect"
          checked={enabled}
          onChange={(on) =>
            // A tone squelch cannot outlive the detector.
            onUpdate(vfo.id, on ? { tones: true } : { tones: false, toneSquelchHz: null })
          }
        />
      }
    >
      {!enabled ? (
        <p className="text-xs text-fg-subtle">CTCSS and DTMF detection is off for this receiver.</p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2" data-testid="ctcss">
            <span className="w-12 text-xs text-fg-muted">CTCSS</span>
            {detected !== null ? (
              <Badge tone="accent" dot title="Sub-tone currently received">
                {formatCtcss(detected)}
              </Badge>
            ) : (
              <span className="text-sm text-fg-subtle">none</span>
            )}
            <Select
              aria-label="Tone squelch"
              title="Tone squelch: the audio opens only while this sub-tone is received (about 0.6 s after the start of a transmission)"
              size="sm"
              className="ml-auto w-28"
              options={SQUELCH_OPTIONS}
              value={squelchHz === null ? "off" : String(squelchHz)}
              onChange={(v) => onUpdate(vfo.id, { toneSquelchHz: v === "off" ? null : Number(v) })}
            />
            {detected !== null && detected !== squelchHz && (
              <Button
                size="sm"
                variant="ghost"
                title="Open the audio only for this sub-tone"
                onClick={() => onUpdate(vfo.id, { toneSquelchHz: detected })}
              >
                Use
              </Button>
            )}
          </div>
          <div className="flex min-w-0 items-start gap-2">
            <span className="w-12 pt-1 text-xs text-fg-muted">DTMF</span>
            <span
              data-testid="dtmf-digits"
              title="Received DTMF digits"
              className="min-h-7 min-w-0 flex-1 font-mono text-xl tracking-widest break-all text-fg"
            >
              {digits === "" ? <span className="text-fg-subtle">·</span> : digits}
            </span>
            <Button
              size="sm"
              variant="ghost"
              disabled={digits === ""}
              onClick={() => void copyDigits(digits)}
            >
              Copy
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={digits === ""}
              onClick={() => audio.clearDigits(vfo.id)}
            >
              Clear
            </Button>
          </div>
        </>
      )}
    </Card>
  );
}
