import { useEffect, useState } from "react";
import { ExpandIcon, IconButton, Slider, Toggle, ZoomInIcon, ZoomOutIcon } from "../ui";
import type { SpectrumInfo, ViewActions } from "../shell/useReceiver";
import { MIN_SPAN_DB, saveLevelSettings, type LevelController } from "./levels";

/** Auto-level readout/slider refresh period; keeps React out of the per-frame path. */
const AUTO_UI_REFRESH_MS = 500;
const ZOOM_FACTOR = 0.5;

interface LevelsUi {
  auto: boolean;
  lowDb: number;
  highDb: number;
}

export function formatSpan(hz: number): string {
  return hz >= 1e6
    ? `${parseFloat((hz / 1e6).toFixed(3))} MHz`
    : `${parseFloat((hz / 1e3).toFixed(1))} kHz`;
}

interface Props {
  controller: LevelController;
  info: SpectrumInfo | null;
  viewActions: { current: ViewActions };
}

/** Display section content: span/zoom and the dB window (auto, floor, ceiling). */
export function DisplayControls({ controller, info, viewActions }: Props) {
  const [levels, setLevels] = useState<LevelsUi>(() => ({
    auto: controller.auto,
    lowDb: Math.round(controller.range.lowDb),
    highDb: Math.round(controller.range.highDb),
  }));

  // Mirror the auto-estimated range into the controls at a low rate.
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (!controller.auto) return;
      const lowDb = Math.round(controller.range.lowDb);
      const highDb = Math.round(controller.range.highDb);
      setLevels((prev) =>
        prev.auto && prev.lowDb === lowDb && prev.highDb === highDb
          ? prev
          : { auto: true, lowDb, highDb },
      );
    }, AUTO_UI_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [controller]);

  const applyManual = (lowDb: number, highDb: number) => {
    controller.setManual(lowDb, highDb);
    setLevels({ auto: false, lowDb: controller.range.lowDb, highDb: controller.range.highDb });
    saveLevelSettings(controller.settings());
  };

  const applyAuto = () => {
    controller.setAuto();
    setLevels((prev) => ({ ...prev, auto: true }));
    saveLevelSettings(controller.settings());
  };

  const dbMin = info?.minDb ?? -120;
  const dbMax = info?.maxDb ?? 0;

  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Display">
      <div className="flex items-center justify-between gap-2">
        <div className="flex flex-col">
          <span className="text-xs font-medium text-fg-muted">Visible span</span>
          <span className="font-mono text-sm text-fg num">
            {info ? formatSpan(info.sampleRate) : "–"}
          </span>
        </div>
        <div className="flex items-center gap-0.5">
          <IconButton
            label="Zoom out"
            variant="secondary"
            onClick={() => viewActions.current.zoom(1 / ZOOM_FACTOR)}
          >
            <ZoomOutIcon />
          </IconButton>
          <IconButton
            label="Zoom in"
            variant="secondary"
            onClick={() => viewActions.current.zoom(ZOOM_FACTOR)}
          >
            <ZoomInIcon />
          </IconButton>
          <IconButton
            label="Full span"
            tip="Full device span (or double-click the display)"
            tooltipAlign="end"
            variant="secondary"
            onClick={() => viewActions.current.reset()}
          >
            <ExpandIcon />
          </IconButton>
        </div>
      </div>
      <Toggle
        label="Automatic levels"
        checked={levels.auto}
        onChange={(auto) => {
          if (auto) applyAuto();
          else applyManual(controller.range.lowDb, controller.range.highDb);
        }}
      />
      <Slider
        label="Floor"
        min={dbMin}
        max={dbMax - MIN_SPAN_DB}
        value={levels.lowDb}
        valueLabel={`${levels.lowDb} dB`}
        onChange={(low) => applyManual(low, Math.max(levels.highDb, low + MIN_SPAN_DB))}
      />
      <Slider
        label="Ceiling"
        min={dbMin + MIN_SPAN_DB}
        max={dbMax}
        value={levels.highDb}
        valueLabel={`${levels.highDb} dB`}
        onChange={(high) => applyManual(Math.min(levels.lowDb, high - MIN_SPAN_DB), high)}
      />
    </div>
  );
}
