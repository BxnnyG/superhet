import { useCallback, useState } from "react";
import { LoadState } from "../admin/common";
import { useLoad } from "../admin/useLoad";
import { describeUserAgent, formatRelative } from "../format";
import { describeHttpError } from "../http";
import { Alert, Badge, Button, Dialog, toasts } from "../ui";
import { createPushApi, type PushApi } from "./api";
import { enablePush, forgetLocalPush, localSubscriptionId, pushSupported } from "./push";

/** Own browser notifications: turn them on for this browser (one click, then the browser asks). */
export function PushDialog({ onClose, api: injected }: { onClose: () => void; api?: PushApi }) {
  const [api] = useState(() => injected ?? createPushApi());
  const state = useLoad(useCallback((s: AbortSignal) => api.get(s), [api]));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const supported = pushSupported();
  const mine = localSubscriptionId();

  const enable = async () => {
    setBusy(true);
    setError(null);
    try {
      await enablePush(api, describeUserAgent(navigator.userAgent));
      toasts.push({ tone: "success", title: "Notifications are on for this browser" });
      state.reload();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: number) => {
    try {
      await api.unsubscribe(id);
      if (id === mine) await forgetLocalPush();
      state.reload();
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not remove it", body: describeHttpError(e) });
    }
  };

  return (
    <Dialog title="Notifications" onClose={onClose} size="lg">
      <p className="text-sm text-fg-muted">
        Get push notifications in this browser when an admin&apos;s notification rule fires. The
        browser asks for permission only after you press the button.
      </p>
      {!supported && <Alert>This browser does not support push notifications.</Alert>}
      {error && <Alert>{error}</Alert>}
      <div>
        <Button variant="primary" onClick={() => void enable()} disabled={!supported || busy}>
          {busy ? "Turning on…" : "Turn on notifications in this browser"}
        </Button>
      </div>
      <LoadState loading={state.loading} error={state.error} onRetry={state.reload} />
      {state.data && state.data.subscriptions.length > 0 && (
        <ul
          className="flex flex-col divide-y divide-border rounded-lg border border-border"
          aria-label="Browsers"
        >
          {state.data.subscriptions.map((s) => (
            <li key={s.id} className="flex items-center gap-3 px-3 py-2.5 text-sm">
              <span className="min-w-0 flex-1 truncate">{s.label ?? "Browser"}</span>
              {s.id === mine && <Badge tone="accent">this browser</Badge>}
              <span className="text-xs text-fg-subtle">{formatRelative(s.createdAtMs)}</span>
              <Button size="sm" onClick={() => void remove(s.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Dialog>
  );
}
