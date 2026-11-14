import { useEffect, useRef, type KeyboardEvent, type PointerEvent } from "react";
import { DRAG_SLOP_PX, dragSteps, keySteps, stepCentre, wheelPixels } from "../spectrum/centreMove";
import { formatFrequency } from "../spectrum/freq";
import { WheelSteps } from "../spectrum/gestures";
import { AlertIcon, LockIcon, PlainButton, Popover, SettingsIcon, cn } from "../ui";
import { droppedNotice, useRecentDrops } from "./dropped";
import { overloadNotice } from "./overload";
import { SourceSettings } from "./SourceSettings";
import type { Receiver } from "./useReceiver";

/**
 * Compact secondary readout of the device centre ("Centre 145.000 · lock"). It opens a popover
 * with the source settings; the VFO frequency is the only big number on screen. It is also a
 * handle for the centre: the wheel steps it (Shift fine, Ctrl coarse), dragging left or right
 * moves it, and the arrow keys step it while the pill has focus. A locked or claimed tuning says
 * why the centre does not move.
 */
export function CentrePill({ r }: { r: Receiver }) {
  const centerHz = r.tuningSnap.display?.centerHz ?? null;
  const spanHz = r.tuningSnap.display?.sampleRate ?? 0;
  const live = useRef({ centerHz, spanHz, tuning: r.tuning });
  useEffect(() => {
    live.current = { centerHz, spanHz, tuning: r.tuning };
  });
  const button = useRef<HTMLButtonElement>(null);
  const drag = useRef<{
    id: number;
    x0: number;
    startHz: number;
    active: boolean;
    blocked: boolean;
    last: number;
  } | null>(null);
  const suppressClick = useRef(false);

  // The wheel listener is native and non-passive so the page does not scroll with it.
  useEffect(() => {
    const el = button.current;
    if (!el) return;
    const wheel = new WheelSteps();
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { centerHz: hz, spanHz: span, tuning } = live.current;
      const steps = wheel.add(wheelPixels(e));
      if (steps === 0 || hz === null) return;
      if (tuning.refuseCentreChange()) return;
      tuning.requestCentre(stepCentre(hz, steps, span, { shift: e.shiftKey, ctrl: e.ctrlKey }));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    const k = keySteps(e);
    if (k === null || centerHz === null) return;
    e.preventDefault();
    e.stopPropagation();
    if (r.tuning.refuseCentreChange()) return;
    r.tuning.requestCentre(stepCentre(centerHz, k.steps, spanHz, k.mods));
  };
  const onPointerDown = (e: PointerEvent<HTMLButtonElement>) => {
    suppressClick.current = false;
    if (e.button !== 0 || centerHz === null) return;
    drag.current = {
      id: e.pointerId,
      x0: e.clientX,
      startHz: centerHz,
      active: false,
      blocked: false,
      last: 0,
    };
  };
  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (d === null || d.id !== e.pointerId || d.blocked) return;
    const dx = e.clientX - d.x0;
    if (!d.active) {
      if (Math.abs(dx) < DRAG_SLOP_PX) return;
      d.active = true;
      suppressClick.current = true;
      if (r.tuning.refuseCentreChange()) {
        d.blocked = true;
        return;
      }
      e.currentTarget.setPointerCapture(e.pointerId);
    }
    const steps = dragSteps(dx);
    if (steps === d.last) return;
    d.last = steps;
    r.tuning.requestCentre(
      stepCentre(d.startHz, steps, spanHz, { shift: e.shiftKey, ctrl: e.ctrlKey }),
    );
  };
  const endDrag = (e: PointerEvent<HTMLButtonElement>) => {
    if (drag.current?.id === e.pointerId) drag.current = null;
  };
  const locked = r.locks.length > 0;
  const overload = overloadNotice(r.tuningSnap.state);
  const dropped = droppedNotice(r.tuningSnap.state);
  const recentDrops = useRecentDrops(r.tuningSnap.state?.droppedBlocks);
  return (
    <Popover
      label="Source settings"
      align="end"
      trigger={({ props }) => (
        <PlainButton
          {...props}
          ref={button}
          onKeyDown={onKeyDown}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onClickCapture={(e) => {
            // A drag ends in a click on the same button; it must not open the popover.
            if (suppressClick.current) {
              suppressClick.current = false;
              e.preventDefault();
              e.stopPropagation();
            }
          }}
          title={
            overload
              ? `${overload.summary} Open the source settings to lower the gain.`
              : dropped && recentDrops
                ? `${dropped.summary} ${dropped.action}`
                : "Device centre and source settings. Wheel, drag or arrow keys move the centre (Shift fine, Ctrl coarse)."
          }
          aria-label={`Centre ${centerHz === null ? "unknown" : formatFrequency(centerHz).slice(0, -4)}${locked ? ", locked" : ""}${overload ? ", ADC overload, reduce gain" : ""}${dropped && recentDrops ? ", dropping samples" : ""}, source settings`}
          className={cn(
            "inline-flex h-8 items-center gap-2 rounded-full border border-border-strong",
            "bg-surface-1 px-3 text-xs text-fg-muted transition-colors duration-150",
            "hover:bg-surface-2 hover:text-fg",
          )}
        >
          <SettingsIcon size={14} className="shrink-0" />
          <span>Centre</span>
          <span className="font-mono text-sm text-fg num">
            {centerHz === null ? "–" : formatFrequency(centerHz).slice(0, -4)}
          </span>
          {locked && <LockIcon size={12} className="shrink-0 text-warn" />}
          {overload && (
            <span
              data-testid="overload-chip"
              className="inline-flex h-5 items-center gap-1 rounded-full bg-warn-soft px-1.5 text-[11px] font-medium text-warn ring-1 ring-warn/20 ring-inset"
            >
              <AlertIcon size={11} className="shrink-0" />
              Overload
            </span>
          )}
          {dropped && recentDrops && (
            <span
              data-testid="dropped-chip"
              className="inline-flex h-5 items-center gap-1 rounded-full bg-warn-soft px-1.5 text-[11px] font-medium text-warn ring-1 ring-warn/20 ring-inset"
            >
              <AlertIcon size={11} className="shrink-0" />
              Dropping
            </span>
          )}
        </PlainButton>
      )}
    >
      <h2 className="mb-3 text-xs font-semibold tracking-wider text-fg-muted uppercase">Source</h2>
      <SourceSettings r={r} />
    </Popover>
  );
}
