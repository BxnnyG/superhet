import { useEffect, useState, type ReactNode } from "react";
import { Alert, Badge, Button, RefreshIcon, cn, toasts, type BadgeTone } from "../ui";
import type { Role } from "../auth/api";
import { ROLE_LABEL } from "../auth/permissions";

/** Title block of a console section: heading, one-line help, actions on the right. */
export function SectionHeader({
  title,
  description,
  actions,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="mb-5 flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
      <div className="min-w-0">
        <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
        {description && <p className="mt-1 max-w-2xl text-sm text-fg-muted">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}

/** Bordered surface for a group of fields or rows. */
export function Card({
  title,
  description,
  children,
  className,
  actions,
}: {
  title?: string;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
  actions?: ReactNode;
}) {
  return (
    <section className={cn("rounded-xl border border-border bg-surface-1", className)}>
      {(title || actions) && (
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-3">
          <div className="min-w-0">
            {title && <h2 className="text-sm font-semibold">{title}</h2>}
            {description && <p className="mt-0.5 text-xs text-fg-muted">{description}</p>}
          </div>
          {actions}
        </div>
      )}
      {children}
    </section>
  );
}

const ROLE_TONE: Record<Role, BadgeTone> = {
  admin: "danger",
  operator: "accent",
  listener: "neutral",
};

export function RoleBadge({ role, guest }: { role: Role; guest?: boolean }) {
  if (guest) return <Badge tone="warn">Guest</Badge>;
  return <Badge tone={ROLE_TONE[role]}>{ROLE_LABEL[role]}</Badge>;
}

/** A loading, error or empty state for a list that has no rows to show. */
export function LoadState({
  loading,
  error,
  onRetry,
}: {
  loading: boolean;
  error: string | null;
  onRetry?: () => void;
}) {
  if (error) {
    return (
      <div className="flex flex-col items-start gap-2 p-4">
        <Alert className="w-full">{error}</Alert>
        {onRetry && (
          <Button size="sm" icon={<RefreshIcon size={14} />} onClick={onRetry}>
            Retry
          </Button>
        )}
      </div>
    );
  }
  if (loading) {
    return (
      <p role="status" className="p-4 text-sm text-fg-muted">
        Loading…
      </p>
    );
  }
  return null;
}

/** Copy text to the clipboard and say so; falls back to a prompt-less toast on failure. */
export async function copyText(text: string, what: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    toasts.push({ key: "copied", tone: "success", title: `${what} copied`, durationMs: 2500 });
    return true;
  } catch {
    toasts.push({
      key: "copied",
      tone: "warn",
      title: "Could not copy",
      body: "Select the text and copy it manually.",
    });
    return false;
  }
}

/** Re-render every `ms` (relative times in lists). */
export function useNow(ms = 15_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(id);
  }, [ms]);
  return now;
}

/** Native text area styled like the text fields (the UI kit has none). */
export function TextArea({
  label,
  value,
  onChange,
  hint,
  rows = 5,
  maxLength,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  hint?: ReactNode;
  rows?: number;
  maxLength?: number;
}) {
  const id = `ta-${label.replace(/\W+/g, "-").toLowerCase()}`;
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <label htmlFor={id} className="text-xs font-medium text-fg-muted">
        {label}
      </label>
      <textarea
        id={id}
        rows={rows}
        maxLength={maxLength}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="focus-ring w-full resize-y rounded-md border border-border-strong bg-surface-1 px-3 py-2 text-sm text-fg placeholder:text-fg-subtle"
      />
      <div className="flex justify-between gap-2 text-xs text-fg-subtle">
        <span>{hint}</span>
        {maxLength !== undefined && (
          <span className="num">
            {value.length} / {maxLength}
          </span>
        )}
      </div>
    </div>
  );
}

/** Save / discard row under a settings form. */
export function SaveBar({
  dirty,
  busy,
  onSave,
  onDiscard,
  error,
}: {
  dirty: boolean;
  busy: boolean;
  onSave: () => void;
  onDiscard: () => void;
  error?: string | null;
}) {
  return (
    <div className="flex flex-col gap-3 border-t border-border px-4 py-3">
      {error && <Alert>{error}</Alert>}
      <div className="flex items-center justify-end gap-2">
        <span className="mr-auto text-xs text-fg-subtle">
          {dirty ? "Unsaved changes" : "All changes saved"}
        </span>
        <Button onClick={onDiscard} disabled={!dirty || busy}>
          Discard
        </Button>
        <Button variant="primary" onClick={onSave} disabled={!dirty || busy}>
          {busy ? "Saving…" : "Save changes"}
        </Button>
      </div>
    </div>
  );
}
