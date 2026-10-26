import { useCallback, useState } from "react";
import { createSelfApi, type MySession, type SelfApi } from "../admin/api";
import { LoadState } from "../admin/common";
import { useLoad } from "../admin/useLoad";
import { describeUserAgent, formatRelative } from "../format";
import { describeHttpError } from "../http";
import { Badge, Button, Dialog, toasts } from "../ui";
import { notifyUnauthorized } from "./session";

/** Own sessions (`/api/v1/me/sessions`): where the account is signed in, with revoke. */
export function MySessionsDialog({
  onClose,
  api: injected,
}: {
  onClose: () => void;
  api?: SelfApi;
}) {
  const [api] = useState(() => injected ?? createSelfApi());
  const load = useCallback((s: AbortSignal) => api.list(s), [api]);
  const sessions = useLoad(load);
  const [busy, setBusy] = useState<string | null>(null);

  const revoke = async (s: MySession) => {
    setBusy(s.sid);
    try {
      await api.revoke(s.sid);
      // Revoking the session this page runs in signs us out.
      if (s.current) {
        notifyUnauthorized();
        return;
      }
      toasts.push({ tone: "success", title: "Session signed out" });
      sessions.reload();
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: "Could not sign out the session",
        body: describeHttpError(e),
      });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog title="My sessions" onClose={onClose} size="lg">
      <p className="text-sm text-fg-muted">
        Browsers and devices where your account is signed in. Sign out the ones you do not
        recognise.
      </p>
      <LoadState loading={sessions.loading} error={sessions.error} onRetry={sessions.reload} />
      {sessions.data && (
        <ul
          className="flex flex-col divide-y divide-border rounded-lg border border-border"
          aria-label="Sessions"
        >
          {sessions.data.map((s) => (
            <li key={s.sid} className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3 py-2.5">
              <div className="min-w-0 flex-1 basis-48">
                <div className="flex flex-wrap items-center gap-1.5 text-sm font-medium">
                  {describeUserAgent(s.userAgent)}
                  {s.current && <Badge tone="success">This device</Badge>}
                </div>
                <p className="text-xs text-fg-muted">
                  <span className="font-mono">{s.ip ?? "address not stored"}</span>
                  {" · "}signed in {formatRelative(s.createdAtMs)} · active{" "}
                  {formatRelative(s.lastSeenMs)}
                </p>
              </div>
              <Button
                size="sm"
                variant={s.current ? "secondary" : "danger"}
                disabled={busy !== null}
                onClick={() => void revoke(s)}
              >
                {s.current ? "Sign out here" : "Sign out"}
              </Button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex justify-end">
        <Button onClick={onClose}>Close</Button>
      </div>
    </Dialog>
  );
}
