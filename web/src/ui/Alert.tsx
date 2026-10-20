import type { ReactNode } from "react";
import { AlertIcon } from "./icons";
import { cn } from "./cn";

export type AlertTone = "danger" | "warn" | "info";

const TONE: Record<AlertTone, string> = {
  danger: "bg-danger-soft text-danger",
  warn: "bg-warn-soft text-warn",
  info: "bg-accent-soft text-accent",
};

/** Inline message box (form errors, notices). Announced to screen readers via role=alert. */
export function Alert({
  tone = "danger",
  children,
  className,
}: {
  tone?: AlertTone;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      role={tone === "info" ? "status" : "alert"}
      className={cn("flex items-start gap-2 rounded-md px-3 py-2 text-sm", TONE[tone], className)}
    >
      <AlertIcon className="mt-0.5 shrink-0" />
      <div className="min-w-0 flex-1 break-words">{children}</div>
    </div>
  );
}
