import type { ReactNode } from "react";
import { cn } from "./cn";

export type BadgeTone = "neutral" | "accent" | "success" | "warn" | "danger";

const TONE: Record<BadgeTone, string> = {
  neutral: "bg-surface-2 text-fg-muted ring-border",
  accent: "bg-accent-soft text-accent ring-accent/20",
  success: "bg-success-soft text-success ring-success/20",
  warn: "bg-warn-soft text-warn ring-warn/20",
  danger: "bg-danger-soft text-danger ring-danger/20",
};

const DOT: Record<BadgeTone, string> = {
  neutral: "bg-fg-subtle",
  accent: "bg-accent",
  success: "bg-success",
  warn: "bg-warn",
  danger: "bg-danger",
};

interface Props {
  tone?: BadgeTone;
  /** Leading status dot. */
  dot?: boolean;
  /** Pulse the dot (connecting states). */
  pulse?: boolean;
  className?: string;
  title?: string;
  children: ReactNode;
}

/** Small pill for modes and status. */
export function Badge({ tone = "neutral", dot, pulse, className, title, children }: Props) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex h-5 items-center gap-1.5 rounded-full px-2 text-[11px] font-medium ring-1 ring-inset",
        TONE[tone],
        className,
      )}
    >
      {dot && (
        <span
          aria-hidden="true"
          className={cn("size-1.5 rounded-full", DOT[tone], pulse && "animate-pulse")}
        />
      )}
      {children}
    </span>
  );
}
