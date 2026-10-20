import { useEffect, useState } from "react";
import type { AudioController } from "../audio/controller";
import { cn } from "../ui";
import { fpsBetween, type SpectrumMetrics } from "./metrics";
import { formatDrift, summarizeDebug, type StatusSummary } from "./statusSummary";

const REFRESH_MS = 500;

interface Props {
  audio: AudioController;
  metrics: SpectrumMetrics;
}

function Stat({
  label,
  value,
  warn,
  title,
  secondary,
}: {
  label: string;
  value: string;
  warn?: boolean;
  title?: string;
  /** Hidden below the shell breakpoint to keep the bar on one line. */
  secondary?: boolean;
}) {
  return (
    <span
      className={cn("inline-flex items-baseline gap-1.5", secondary && "max-shell:hidden")}
      title={title}
    >
      <span className="text-fg-subtle">{label}</span>
      <span className={warn ? "text-warn" : "text-fg-muted"}>{value}</span>
    </span>
  );
}

/** Compact, muted bottom bar; sampled at a low rate so React never renders per frame. */
export function StatusBar({ audio, metrics }: Props) {
  const [summary, setSummary] = useState<StatusSummary>(() => summarizeDebug(null));
  const [fps, setFps] = useState(0);

  useEffect(() => {
    let prevFrames = metrics.frames;
    let prevAt = performance.now();
    const timer = window.setInterval(() => {
      const now = performance.now();
      setFps(fpsBetween(prevFrames, metrics.frames, now - prevAt));
      prevFrames = metrics.frames;
      prevAt = now;
      setSummary(summarizeDebug(audio.getDebug()));
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio, metrics]);

  return (
    <footer
      className="flex h-7 shrink-0 items-center gap-3 overflow-x-auto border-t border-border bg-surface-1 px-4 font-mono text-[11px] whitespace-nowrap num"
      aria-label="Status"
    >
      <Stat secondary label="buffer" value={summary.buffer ?? "–"} />
      <Stat secondary label="drift" value={formatDrift(summary.driftPct)} />
      <Stat
        label="latency"
        value={summary.latencyMs === null ? "–" : `${Math.round(summary.latencyMs)} ms`}
        title="Capture to speaker estimate (includes server/browser clock skew)"
      />
      <Stat label="underruns" value={String(summary.underruns)} warn={summary.underruns > 0} />
      <Stat
        secondary
        label="dropped"
        value={`${summary.lostSamples} smp`}
        warn={summary.lostSamples > 0}
        title="Audio samples replaced by silence after lost frames"
      />
      <Stat label="fps" value={fps.toFixed(0)} />
    </footer>
  );
}
