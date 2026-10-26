import {
  ArrowLeftIcon,
  ArrowRightIcon,
  Badge,
  BookmarkIcon,
  Button,
  FrequencyReadout,
  IconButton,
  PauseIcon,
  PlusIcon,
  Select,
} from "../ui";
import {
  STEPS_HZ,
  VFO_PAUSED_FREE_HINT,
  VFO_PAUSED_HINT,
  formatStep,
  snapFrequency,
  stepFor,
  vfoColor,
} from "../vfo/model";
import { BandwidthSelect, ModeSelect, SquelchControl } from "../vfo/controls";
import { SMeter } from "../vfo/SMeter";
import type { VfoPatch } from "../vfo/api";
import { CentrePill } from "./CentrePill";
import { PHONE_QUERY, useMediaQuery } from "./useMediaQuery";
import type { Receiver } from "./useReceiver";

/**
 * The one primary tuning strip, directly above the spectrum. It always edits the selected VFO:
 * frequency (digit wheel / typed), step, nudge, bookmark on the left, mode, bandwidth and
 * squelch with the live S-meter in the middle, the device centre pill on the right. Without a VFO
 * it offers to create one instead of silently tuning the device centre.
 */
export function TuningStrip({ r }: { r: Receiver }) {
  const vfo = r.selected;
  const wide = useMediaQuery(PHONE_QUERY);
  const centerHz = r.tuningSnap.display?.centerHz ?? null;
  const locked = r.locks.length > 0;

  return (
    <section
      aria-label="Tuning"
      className="shrink-0 border-b border-border bg-surface-1 px-3 shell:px-4"
    >
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 py-2 shell:min-h-16 shell:py-1">
        {vfo === null ? (
          <div className="flex min-w-0 flex-1 items-center gap-3">
            <Button
              variant="primary"
              icon={<PlusIcon />}
              disabled={!r.vfoSnap.loaded || centerHz === null}
              title="Create a receiver at the display centre (or click the spectrum)"
              onClick={() => r.createVfoAt(centerHz)}
            >
              Create receiver
            </Button>
            <p className="min-w-0 truncate text-sm text-fg-muted">
              {r.vfoSnap.loaded ? "Click the spectrum or press N to add a VFO" : "Loading…"}
            </p>
          </div>
        ) : (
          <ActiveVfo r={r} vfo={vfo} wide={wide} locked={locked} />
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2 max-phone:hidden max-shell:order-2 shell:order-last">
          <CentrePill r={r} />
        </div>
      </div>
    </section>
  );
}

function ActiveVfo({
  r,
  vfo,
  wide,
  locked,
}: {
  r: Receiver;
  vfo: NonNullable<Receiver["selected"]>;
  wide: boolean;
  locked: boolean;
}) {
  const stepHz = stepFor(r.step, vfo.mode);
  const nudge = (dir: 1 | -1) =>
    r.vfoCtl.update(vfo.id, { freqHz: snapFrequency(vfo.freqHz + dir * stepHz, stepHz) });
  const onUpdate = (id: number, patch: VfoPatch) => r.vfoCtl.update(id, patch);
  return (
    <>
      <div
        role="group"
        aria-label="Active VFO"
        className="flex min-w-0 items-center gap-2 shell:shrink-0"
      >
        <span
          aria-hidden="true"
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: vfoColor(vfo.id) }}
        />
        <span className="shrink-0 text-xs font-medium text-fg-muted num max-2xl:sr-only">
          VFO {vfo.id}
        </span>
        <IconButton
          label="Down one step"
          variant="secondary"
          className="max-phone:size-11"
          onClick={() => nudge(-1)}
        >
          <ArrowLeftIcon />
        </IconButton>
        <FrequencyReadout
          key={vfo.id}
          label="VFO frequency"
          valueHz={vfo.freqHz}
          onChange={(hz) => onUpdate(vfo.id, { freqHz: hz })}
        />
        <IconButton
          label="Up one step"
          variant="secondary"
          className="max-phone:size-11"
          onClick={() => nudge(1)}
        >
          <ArrowRightIcon />
        </IconButton>
        <Select
          aria-label="Tuning step"
          size="sm"
          className="w-28 shrink-0 max-phone:hidden"
          value={String(r.step)}
          onChange={(v) => r.setStep(v === "auto" ? "auto" : Number(v))}
          options={[
            {
              value: "auto",
              label: `Auto ${formatStep(stepFor("auto", vfo.mode)).replace(" kHz", "k")}`,
            },
            ...STEPS_HZ.map((s) => ({ value: String(s), label: formatStep(s) })),
          ]}
        />
        <IconButton
          label="Bookmark this frequency"
          className="max-phone:size-11"
          tip="Bookmark this frequency (B)"
          onClick={r.startBookmark}
        >
          <BookmarkIcon />
        </IconButton>
        {vfo.status === "paused" && (
          <Badge tone="warn" title={locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT}>
            <PauseIcon size={10} />
            paused
          </Badge>
        )}
      </div>
      {wide && (
        <div
          role="group"
          aria-label="Demodulator"
          className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 max-shell:order-3 max-shell:basis-full shell:basis-[31rem] shell:flex-1 shell:flex-nowrap"
        >
          <ModeSelect vfo={vfo} onUpdate={onUpdate} />
          <BandwidthSelect vfo={vfo} onUpdate={onUpdate} />
          <div className="flex min-w-32 flex-1 flex-col gap-0.5 shell:max-w-72">
            <SquelchControl
              vfo={vfo}
              audio={r.audio}
              autoSquelch={r.autoSquelch}
              onUpdate={onUpdate}
            />
            <SMeter audio={r.audio} vfoId={vfo.id} inline />
          </div>
        </div>
      )}
    </>
  );
}
