import type { ReactNode } from "react";
import { cn } from "./cn";

/** Label above a non-input control (segmented control, button row). */
export function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <span className="text-xs font-medium text-fg-muted">{label}</span>
      {children}
    </div>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-1 rounded-lg border border-dashed border-border-strong px-4 py-5 text-center">
      <p className="text-sm font-medium text-fg">{title}</p>
      {children && <p className="text-xs text-fg-muted">{children}</p>}
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-border-strong bg-surface-2 px-1 font-mono text-[10px] text-fg-muted">
      {children}
    </kbd>
  );
}
