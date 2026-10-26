import { useCallback, useState } from "react";
import { describeHttpError } from "../http";
import { NumberField, toasts } from "../ui";
import type { AdminApi, Limits } from "./api";
import { Card, LoadState, SaveBar, SectionHeader } from "./common";
import { useDraft } from "./useDraft";
import { useLoad } from "./useLoad";

export function LimitsSection({ api }: { api: AdminApi }) {
  const load = useCallback((s: AbortSignal) => api.getLimits(s), [api]);
  const loaded = useLoad(load);
  const [saved, setSaved] = useState<Limits | null>(null);
  const draft = useDraft(saved ?? loaded.data);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const l = draft.value;

  const save = async () => {
    if (!l) return;
    setBusy(true);
    setError(null);
    try {
      setSaved(await api.putLimits(l));
      toasts.push({ tone: "success", title: "Limits saved" });
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Limits"
        description="How many receivers (VFOs) each person may have open at once. Admins have no limit of their own. Existing receivers are not closed when a limit drops."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {l && (
        <div className="flex flex-col gap-6">
          <Card
            title="Receivers per person"
            description="0 means only the daemon's own cap applies."
          >
            <div className="grid gap-4 p-4 sm:grid-cols-2">
              <NumberField
                label="Listeners"
                min={0}
                max={64}
                value={l.listenerMaxVfos}
                onCommit={(listenerMaxVfos) => draft.set({ ...l, listenerMaxVfos })}
              />
              <NumberField
                label="Operators"
                min={0}
                max={64}
                value={l.operatorMaxVfos}
                onCommit={(operatorMaxVfos) => draft.set({ ...l, operatorMaxVfos })}
              />
            </div>
            <p className="px-4 pb-4 text-xs text-fg-subtle">
              Guests have their own limit under Guest access. The daemon cap is the{" "}
              <code className="font-mono">SUPERHET_MAX_VFOS</code> setting of the device service
              (default 8, at most 64).
            </p>
          </Card>
          <Card className="sticky bottom-4 z-10 shadow-pop">
            <SaveBar
              dirty={draft.dirty}
              busy={busy}
              error={error}
              onSave={() => void save()}
              onDiscard={() => {
                draft.discard();
                setError(null);
              }}
            />
          </Card>
        </div>
      )}
    </div>
  );
}
