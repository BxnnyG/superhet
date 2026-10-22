import { useSyncExternalStore } from "react";
import { Button } from "./Button";
import { cn } from "./cn";
import { IconButton } from "./IconButton";
import { AlertIcon, CheckIcon, CloseIcon, SignalIcon } from "./icons";
import { toasts, type ToastStore, type ToastTone } from "./toast";

const TONE: Record<ToastTone, string> = {
  info: "text-accent",
  success: "text-success",
  warn: "text-warn",
  danger: "text-danger",
};

function ToneIcon({ tone }: { tone: ToastTone }) {
  if (tone === "success") return <CheckIcon className="mt-0.5 shrink-0" />;
  if (tone === "info") return <SignalIcon className="mt-0.5 shrink-0" />;
  return <AlertIcon className="mt-0.5 shrink-0" />;
}

/**
 * Stack of non-blocking notices (bottom right, above the status bar). The region is a polite live
 * region; danger toasts are announced as alerts. Mount once near the app root.
 */
export function Toaster({ store = toasts }: { store?: ToastStore }) {
  const items = useSyncExternalStore(store.subscribe, store.getSnapshot);
  return (
    <div
      aria-label="Notifications"
      role="region"
      className="pointer-events-none fixed right-3 bottom-10 z-[70] flex w-80 max-w-[calc(100vw-1.5rem)] flex-col gap-2"
    >
      {items.map((t) => (
        <div
          key={t.id}
          role={t.tone === "danger" ? "alert" : "status"}
          className="pointer-events-auto flex items-start gap-2 rounded-lg border border-border bg-surface-1 p-3 shadow-pop"
        >
          <span className={cn("flex", TONE[t.tone])}>
            <ToneIcon tone={t.tone} />
          </span>
          <div className="min-w-0 flex-1 break-words">
            <p className="text-sm font-medium text-fg">{t.title}</p>
            {t.body && <p className="mt-0.5 text-xs text-fg-muted">{t.body}</p>}
            {t.action && (
              <Button
                size="sm"
                variant="secondary"
                className="mt-2"
                onClick={() => {
                  t.action?.onClick();
                  store.dismiss(t.id);
                }}
              >
                {t.action.label}
              </Button>
            )}
          </div>
          <IconButton label="Dismiss notification" size="sm" onClick={() => store.dismiss(t.id)}>
            <CloseIcon size={14} />
          </IconButton>
        </div>
      ))}
    </div>
  );
}
