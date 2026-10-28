import { useMemo, useRef, useState, type ChangeEvent, type PointerEvent } from "react";
import { usePermissions } from "../auth/context";
import { NEEDS_OPERATOR } from "../auth/permissions";
import type { Receiver } from "../shell/useReceiver";
import { formatFrequency } from "../spectrum/freq";
import {
  Alert,
  ArrowLeftIcon,
  Badge,
  Button,
  ChevronRightIcon,
  ConfirmDialog,
  EmptyState,
  IconButton,
  PlainButton,
  PlusIcon,
  SearchIcon,
  SegmentedControl,
  Select,
  TextField,
  Toggle,
  TrashIcon,
  cn,
  toasts,
  type BadgeTone,
} from "../ui";
import { planFromFile } from "./import";
import { activeRaster, rasterChannel, rasterCount, rasterName } from "./lookup";
import {
  COUNTRY_CODES,
  browserRegion,
  countryName,
  describeRegions,
  regionMatches,
} from "./regions";
import {
  LICENCE_LABEL,
  modeOf,
  needsHf,
  normalizeNumber,
  planDocument,
  type Licence,
  type Plan,
  type PlanChannel,
} from "./schema";
import { MODE_LABEL } from "../vfo/model";

const LICENCE_TONE: Record<Licence, BadgeTone> = {
  "licence-free": "success",
  licensed: "warn",
  "receive-only": "neutral",
};

/** Press and hold this long on a channel to open it in a new VFO (touch has no Alt key). */
export const LONG_PRESS_MS = 500;

const mhz = (hz: number) => formatFrequency(hz).replace(/\s+MHz$/, "");

/** "446.006 - 446.194 MHz" style summary of what a plan covers. */
export function planSpan(plan: Plan): string {
  const freqs = [
    ...plan.channels.map((c) => c.freqHz),
    ...plan.rasters.flatMap((r) => [r.startHz, r.endHz]),
  ];
  if (freqs.length === 0) return "";
  const lo = Math.min(...freqs);
  const hi = Math.max(...freqs);
  return lo === hi
    ? `${formatFrequency(lo)} MHz`
    : `${formatFrequency(lo)} - ${formatFrequency(hi)} MHz`;
}

function matchesQuery(plan: Plan, q: string): boolean {
  if (q === "") return true;
  const hay = [plan.name, plan.service, plan.chip, ...plan.aliases, ...plan.regions]
    .join(" ")
    .toLowerCase();
  return q.split(/\s+/).every((w) => hay.includes(w));
}

/**
 * Channels tab: service cards per plan (the ones of the user's region first), a channel grid per
 * plan, the region and stepping settings and the import of custom plans.
 */
export function ChannelsPanel({ r }: { r: Receiver }) {
  const snap = r.channelSnap;
  const perms = usePermissions();
  const [openId, setOpenId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [deleting, setDeleting] = useState<Plan | null>(null);
  const [busy, setBusy] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const plan = openId === null ? null : (snap.plans.find((p) => p.id === openId) ?? null);

  const q = query.trim().toLowerCase();
  const visible = useMemo(() => snap.plans.filter((p) => matchesQuery(p, q)), [snap.plans, q]);
  const mine = visible.filter((p) => regionMatches(p.regions, snap.region));
  const other = visible.filter((p) => !regionMatches(p.regions, snap.region));

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    try {
      const parsed = planFromFile(await file.text(), file.name, snap.region);
      const error = await r.channels.add(planDocument(parsed));
      toasts.push(
        error === null
          ? {
              tone: "success",
              title: `Imported "${parsed.name}"`,
              body: `${parsed.channels.length} channels`,
            }
          : { tone: "danger", title: "Could not store the plan", body: error },
      );
    } catch (err) {
      toasts.push({
        tone: "danger",
        title: "Could not import the file",
        body: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const confirmDelete = async () => {
    if (deleting === null) return;
    setBusy(true);
    const error = await r.channels.remove(deleting);
    setBusy(false);
    setDeleting(null);
    if (error !== null)
      toasts.push({ tone: "danger", title: "Could not delete the plan", body: error });
    else if (openId === deleting.id) setOpenId(null);
  };

  if (plan !== null) {
    return (
      <div className="flex flex-col gap-3 px-4 py-3">
        <PlanView
          r={r}
          plan={plan}
          onBack={() => setOpenId(null)}
          onDelete={perms.isOperator ? () => setDeleting(plan) : null}
        />
        {deleting !== null && (
          <ConfirmDialog
            title="Delete this channel plan?"
            confirmLabel="Delete"
            confirmVariant="danger"
            busy={busy}
            onConfirm={() => void confirmDelete()}
            onCancel={() => setDeleting(null)}
          >
            "{deleting.name}" is removed for everybody. The built-in plans are not affected.
          </ConfirmDialog>
        )}
      </div>
    );
  }

  const detected = browserRegion();
  return (
    <div className="flex flex-col gap-3 px-4 py-3">
      <TextField
        label="Find a service"
        type="search"
        placeholder="PMR, marine, DAB, CB…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        trailing={<SearchIcon size={14} className="mr-2 text-fg-subtle" />}
      />
      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-end gap-3">
        <Select
          label="Region"
          size="sm"
          value={snap.prefs.region ?? "auto"}
          onChange={(v) => r.channels.setRegion(v === "auto" ? null : v)}
          options={[
            {
              value: "auto",
              label: detected ? `Browser (${countryName(detected)})` : "Browser (unknown)",
            },
            ...COUNTRY_CODES.map((c) => ({ value: c, label: `${countryName(c)} (${c})` })),
          ]}
        />
        <Toggle
          label="Step by channel"
          checked={snap.prefs.stepByChannel}
          onChange={(on) => r.channels.setStepByChannel(on)}
          className="pb-1.5"
        />
      </div>
      <p className="text-xs text-fg-subtle">
        With a VFO on a channel, the arrows and the left / right keys step channels instead of kHz.
        Tap a channel to tune the active VFO, Alt-click or press and hold for a new one.
      </p>

      {snap.error !== null && (
        <Alert tone="warn">Custom plans are not available: {snap.error}</Alert>
      )}
      {snap.skipped > 0 && (
        <Alert tone="warn">
          {snap.skipped} stored {snap.skipped === 1 ? "plan" : "plans"} could not be read by this
          version and {snap.skipped === 1 ? "is" : "are"} hidden.
        </Alert>
      )}

      {visible.length === 0 && (
        <EmptyState title="No matching service">Try another word or clear the search.</EmptyState>
      )}
      {mine.length > 0 && (
        <PlanGroup
          title={snap.region ? `For ${countryName(snap.region)}` : "Worldwide"}
          plans={mine}
          onOpen={setOpenId}
        />
      )}
      {other.length > 0 && (
        <PlanGroup
          title={mine.length > 0 || snap.region ? "Other regions" : "All services"}
          plans={other}
          onOpen={setOpenId}
        />
      )}

      <div className="flex items-center gap-2 border-t border-border pt-3">
        <Button
          variant="secondary"
          size="sm"
          icon={<PlusIcon />}
          disabled={!perms.isOperator}
          title={perms.isOperator ? "Import a plan JSON or a CHIRP CSV" : NEEDS_OPERATOR}
          onClick={() => fileRef.current?.click()}
        >
          Import plan…
        </Button>
        <p className="min-w-0 text-xs text-fg-subtle">
          Plan JSON or CHIRP CSV, shared with all users.
        </p>
        <input
          ref={fileRef}
          type="file"
          accept=".json,.csv,application/json,text/csv"
          className="sr-only"
          tabIndex={-1}
          aria-label="Import a channel plan file"
          onChange={(e) => void onFile(e)}
        />
      </div>
    </div>
  );
}

function PlanGroup({
  title,
  plans,
  onOpen,
}: {
  title: string;
  plans: readonly Plan[];
  onOpen: (id: string) => void;
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-1.5">
      <h3 className="text-[11px] font-semibold tracking-wide text-fg-subtle uppercase">{title}</h3>
      <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
        {plans.map((p) => (
          <li key={p.id}>
            <ServiceCard plan={p} onOpen={() => onOpen(p.id)} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function ServiceCard({ plan, onOpen }: { plan: Plan; onOpen: () => void }) {
  const count =
    plan.channels.length > 0
      ? `${plan.channels.length} ${plan.channels.length === 1 ? "channel" : "channels"}`
      : plan.rasters.map((r) => r.label).join(" / ");
  return (
    <PlainButton
      onClick={onOpen}
      className="group flex min-h-14 w-full items-center gap-2 rounded-lg border border-border bg-surface-1 px-3 py-2 text-left transition-colors duration-150 hover:border-border-strong hover:bg-surface-2"
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-sm font-medium text-fg">{plan.name}</span>
          {plan.builtin ? null : <Badge tone="accent">custom</Badge>}
        </span>
        <span className="truncate text-xs text-fg-muted num">
          {count} · {planSpan(plan)}
        </span>
        <span className="flex flex-wrap items-center gap-1">
          <Badge tone={LICENCE_TONE[plan.licence]}>{LICENCE_LABEL[plan.licence]}</Badge>
          {needsHf(plan) && <Badge tone="warn">HF</Badge>}
          <span className="text-[11px] text-fg-subtle">{describeRegions(plan.regions)}</span>
        </span>
      </span>
      <ChevronRightIcon size={16} className="shrink-0 text-fg-subtle group-hover:text-fg" />
    </PlainButton>
  );
}

function PlanView({
  r,
  plan,
  onBack,
  onDelete,
}: {
  r: Receiver;
  plan: Plan;
  onBack: () => void;
  onDelete: (() => void) | null;
}) {
  const [filter, setFilter] = useState("");
  const perms = usePermissions();
  const snap = r.channelSnap;
  const raster = activeRaster(plan, snap.prefs.rasters);
  const dab = plan.channels.some((c) => modeOf(plan, c) === "dab");
  const f = filter.trim().toLowerCase();
  const shown = plan.channels.filter(
    (c) =>
      f === "" ||
      c.number.toLowerCase().includes(f) ||
      (c.name ?? "").toLowerCase().includes(f) ||
      mhz(c.freqHz).includes(f),
  );
  const current = r.channelMatch;
  const isCurrent = (c: PlanChannel) =>
    current !== null &&
    current.plan.id === plan.id &&
    current.raster === null &&
    normalizeNumber(current.channel.number) === normalizeNumber(c.number);

  return (
    <>
      <div className="flex items-center gap-1">
        <IconButton label="Back to all services" size="sm" onClick={onBack}>
          <ArrowLeftIcon />
        </IconButton>
        <h3 className="min-w-0 flex-1 truncate text-sm font-semibold">{plan.name}</h3>
        {onDelete !== null && !plan.builtin && (
          <IconButton label="Delete this plan" size="sm" onClick={onDelete}>
            <TrashIcon />
          </IconButton>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <Badge tone={LICENCE_TONE[plan.licence]}>{LICENCE_LABEL[plan.licence]}</Badge>
        {plan.mode !== "dab" && <Badge>{MODE_LABEL[plan.mode]}</Badge>}
        <span className="text-xs text-fg-subtle">{describeRegions(plan.regions)}</span>
      </div>
      {plan.licenceNote && <p className="text-xs text-fg-muted">{plan.licenceNote}</p>}
      {plan.note && <p className="text-xs text-fg-muted">{plan.note}</p>}
      {needsHf(plan) && (
        <Alert tone="warn">
          HF: for good reception an RTL-SDR needs an upconverter or direct sampling here (its tuner
          starts at about 24 MHz and is poor below 30 MHz).
        </Alert>
      )}
      {dab && (
        <Alert tone="info">
          Selecting a block starts the DAB plugin on it (DAB tab): it takes the whole device and
          pauses receivers outside the 1.536 MHz block until you stop it
          {perms.isOperator ? "." : "; only operators and admins can start it."}
        </Alert>
      )}

      {raster !== null && (
        <div className="flex flex-col gap-2 rounded-lg border border-border bg-surface-1 p-3">
          <p className="text-xs font-medium text-fg-muted">Channel raster for stepping</p>
          {plan.rasters.length > 1 && (
            <SegmentedControl
              aria-label="Channel raster"
              size="sm"
              value={raster.id}
              onChange={(id) => r.channels.setRaster(plan.id, id)}
              options={plan.rasters.map((x) => ({ value: x.id, label: x.label }))}
            />
          )}
          <p className="text-xs text-fg-subtle num">
            {formatFrequency(raster.startHz)} - {formatFrequency(raster.endHz)} MHz ·{" "}
            {rasterCount(raster)} channels, first {rasterName(raster, 0)}, second{" "}
            {rasterName(raster, 1)}. While a VFO sits on this grid the arrows step along it.
          </p>
          <Button
            variant="secondary"
            size="sm"
            className="self-start"
            onClick={() => r.tuneChannel(plan, rasterChannel(raster, 0), false)}
          >
            Tune to {rasterName(raster, 0)}
          </Button>
        </div>
      )}

      {plan.channels.length > 0 && (
        <>
          {plan.channels.length > 12 && (
            <TextField
              label="Filter channels"
              type="search"
              placeholder="Number, name or MHz"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          )}
          {shown.length === 0 ? (
            <EmptyState title="No such channel">Clear the filter to see all channels.</EmptyState>
          ) : (
            <ul
              aria-label={`${plan.name} channels`}
              className="m-0 grid list-none grid-cols-3 gap-1.5 p-0"
            >
              {shown.map((c) => (
                <li key={c.number} className="min-w-0">
                  <ChannelButton
                    plan={plan}
                    channel={c}
                    current={isCurrent(c)}
                    onTune={(newVfo) => r.tuneChannel(plan, c, newVfo)}
                  />
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      {plan.source && <p className="text-[11px] text-fg-subtle">Source: {plan.source}</p>}
    </>
  );
}

function ChannelButton({
  plan,
  channel,
  current,
  onTune,
}: {
  plan: Plan;
  channel: PlanChannel;
  current: boolean;
  onTune: (newVfo: boolean) => void;
}) {
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const longPressed = useRef(false);
  const cancel = () => {
    clearTimeout(timer.current);
    timer.current = undefined;
  };
  const onPointerDown = (e: PointerEvent) => {
    if (e.pointerType === "mouse") return; // mouse users have Alt-click
    longPressed.current = false;
    cancel();
    timer.current = setTimeout(() => {
      longPressed.current = true;
      onTune(true);
    }, LONG_PRESS_MS);
  };
  const dab = modeOf(plan, channel) === "dab";
  const details = [
    channel.name,
    channel.ctcssHz !== null ? `CTCSS ${channel.ctcssHz.toFixed(1)} Hz` : null,
    channel.note,
  ].filter((t): t is string => t !== null);
  return (
    <PlainButton
      aria-pressed={current}
      aria-label={`${plan.chip} ${channel.number}, ${mhz(channel.freqHz)} MHz${
        channel.name ? `, ${channel.name}` : ""
      }`}
      title={[`${mhz(channel.freqHz)} MHz`, ...details].join("\n")}
      onPointerDown={onPointerDown}
      onPointerUp={cancel}
      onPointerLeave={cancel}
      onPointerCancel={cancel}
      onContextMenu={(e) => {
        if (timer.current !== undefined || longPressed.current) e.preventDefault();
      }}
      onClick={(e) => {
        if (longPressed.current) {
          longPressed.current = false;
          return;
        }
        onTune(e.altKey && !dab);
      }}
      className={cn(
        "flex min-h-12 w-full flex-col items-center justify-center rounded-md border px-1 py-1 transition-colors duration-150",
        "select-none [-webkit-touch-callout:none]",
        current
          ? "border-accent/60 bg-accent-soft text-fg"
          : "border-border bg-surface-1 text-fg hover:border-border-strong hover:bg-surface-2",
      )}
    >
      <span className="max-w-full truncate text-sm font-semibold">{channel.number}</span>
      <span className="text-[11px] text-fg-muted num">{mhz(channel.freqHz)}</span>
    </PlainButton>
  );
}
