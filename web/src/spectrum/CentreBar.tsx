import { useEffect, useRef, type KeyboardEvent, type PointerEvent } from "react";
import { PlainButton, cn } from "../ui";
import { DRAG_SLOP_PX, keySteps, panCentre, previewShiftPx, stepCentre } from "./centreMove";
import { formatFrequency } from "./freq";
import type { TuningController } from "./tuning";
import type { Span } from "./view";

interface Props {
  tuning: Pick<TuningController, "getSnapshot" | "requestCentre" | "refuseCentreChange">;
  /** The span on screen (what the pointer maps to), null before the first frame. */
  getSpan: () => Span | null;
  /** The element that shows the picture; it follows the pointer while dragging. */
  getPreview: () => HTMLElement | null;
}

/** How long the preview shift stays after release, until the first frame of the new centre. */
const SETTLE_MS = 300;

interface Drag {
  id: number;
  x0: number;
  startHz: number;
  spanHz: number;
  width: number;
  active: boolean;
  blocked: boolean;
  hz: number;
}

/**
 * Thin handle on the top edge of the waterfall: drag it left or right and the device centre
 * pans with the picture. The picture follows the pointer live (a transform on the canvas, no React
 * render per move) and the new centre is sent on release. A locked or claimed tuning says why
 * nothing moves. Works with touch (pointer events) and the arrow keys.
 */
export function CentreBar({ tuning, getSpan, getPreview }: Props) {
  const chip = useRef<HTMLSpanElement>(null);
  const drag = useRef<Drag | null>(null);
  const settle = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(settle.current), []);

  const showChip = (text: string | null) => {
    const el = chip.current;
    if (!el) return;
    el.hidden = text === null;
    if (text !== null) el.textContent = text;
  };
  const shiftPicture = (px: number) => {
    const el = getPreview();
    if (el) el.style.transform = px === 0 ? "" : `translate3d(${px}px,0,0)`;
  };

  const onPointerDown = (e: PointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    const state = tuning.getSnapshot().display;
    const span = getSpan();
    if (!state || !span) return;
    clearTimeout(settle.current);
    shiftPicture(0);
    drag.current = {
      id: e.pointerId,
      x0: e.clientX,
      startHz: state.centerHz,
      spanHz: span.endHz - span.startHz,
      width: e.currentTarget.getBoundingClientRect().width,
      active: false,
      blocked: false,
      hz: state.centerHz,
    };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const d = drag.current;
    if (d === null || d.id !== e.pointerId || d.blocked) return;
    const dx = e.clientX - d.x0;
    if (!d.active) {
      if (Math.abs(dx) < DRAG_SLOP_PX) return;
      d.active = true;
      if (tuning.refuseCentreChange()) {
        d.blocked = true;
        return;
      }
    }
    d.hz = panCentre(d.startHz, dx, d.width, d.spanHz);
    shiftPicture(previewShiftPx(d.hz - d.startHz, d.width, d.spanHz));
    showChip(`${formatFrequency(d.hz).slice(0, -4)} MHz`);
  };

  const finish = (e: PointerEvent<HTMLButtonElement>, commit: boolean) => {
    const d = drag.current;
    if (d === null || d.id !== e.pointerId) return;
    drag.current = null;
    showChip(null);
    if (commit && d.active && !d.blocked && d.hz !== d.startHz) {
      tuning.requestCentre(d.hz);
      // Keep the shifted picture until the new centre's frames arrive.
      settle.current = setTimeout(() => shiftPicture(0), SETTLE_MS);
    } else {
      shiftPicture(0);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    const k = keySteps(e);
    const state = tuning.getSnapshot().display;
    if (k === null || !state) return;
    e.preventDefault();
    e.stopPropagation();
    if (tuning.refuseCentreChange()) return;
    tuning.requestCentre(stepCentre(state.centerHz, k.steps, state.sampleRate, k.mods));
  };

  return (
    <PlainButton
      aria-label="Move the device centre: drag left or right, or use the arrow keys (Shift fine, Ctrl coarse)"
      title="Drag to move the centre frequency"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(e) => finish(e, true)}
      onPointerCancel={(e) => finish(e, false)}
      onKeyDown={onKeyDown}
      className={cn(
        "group/pan focus-ring absolute inset-x-0 top-0 z-10 flex h-4 cursor-ew-resize touch-none",
        "items-center justify-center [@media(pointer:coarse)]:h-7",
      )}
    >
      <span className="h-1 w-16 rounded-full bg-zinc-700/80 transition-colors duration-150 group-hover/pan:bg-indigo-400 group-focus-visible/pan:bg-indigo-400" />
      <span
        ref={chip}
        hidden
        data-testid="centre-bar-chip"
        className="pointer-events-none absolute top-full mt-1 rounded-md bg-surface-2 px-2 py-0.5 font-mono text-xs text-fg shadow-pop num"
      />
    </PlainButton>
  );
}
