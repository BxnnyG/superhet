import { useCallback, useState } from "react";
import { describeHttpError } from "../http";
import { Alert, Chip, NumberField, Toggle, toasts } from "../ui";
import { ALL_MODES, type AdminApi, type GuestPolicy } from "./api";
import { Card, LoadState, SaveBar, SectionHeader, TextArea } from "./common";
import { useDraft } from "./useDraft";
import { useLoad } from "./useLoad";

const MODE_LABEL: Record<string, string> = {
  wfm: "WFM",
  nfm: "NFM",
  am: "AM",
  usb: "USB",
  lsb: "LSB",
  cw: "CW",
};

/** MHz text of a frequency in Hz (`145.5`), and back. */
export const formatMhzText = (hz: number): string => String(Number((hz / 1e6).toFixed(6)));
export const parseMhzText = (text: string): number | null => {
  const n = Number(text.trim().replace(",", "."));
  return text.trim() === "" || !Number.isFinite(n) ? null : Math.round(n * 1e6);
};

/** What is wrong with the policy before it is sent (the server validates again). */
export function policyProblem(p: GuestPolicy): string | null {
  if (p.freqMinHz !== null && p.freqMaxHz !== null && p.freqMinHz >= p.freqMaxHz) {
    return "The lowest guest frequency must be below the highest.";
  }
  if (p.allowedModes.length === 0) {
    return "Allow at least one mode, otherwise guests cannot open a receiver.";
  }
  return null;
}

export function GuestSection({ api }: { api: AdminApi }) {
  const load = useCallback((s: AbortSignal) => api.getGuestPolicy(s), [api]);
  const loaded = useLoad(load);
  const [saved, setSaved] = useState<GuestPolicy | null>(null);
  const draft = useDraft(saved ?? loaded.data);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const p = draft.value;
  const problem = p ? policyProblem(p) : null;

  const save = async () => {
    if (!p || problem) return;
    setBusy(true);
    setError(null);
    try {
      const next = await api.putGuestPolicy(p);
      setSaved(next);
      toasts.push({
        tone: "success",
        title: "Guest policy saved",
        body: next.enabled ? undefined : "Guest access is off; guest sessions were ended.",
      });
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  const update = (patch: Partial<GuestPolicy>) => p && draft.set({ ...p, ...patch });
  const toggleMode = (mode: string) =>
    p &&
    update({
      allowedModes: p.allowedModes.includes(mode)
        ? p.allowedModes.filter((m) => m !== mode)
        : ALL_MODES.filter((m) => m === mode || p.allowedModes.includes(m)),
    });

  return (
    <div>
      <SectionHeader
        title="Guest access"
        description="Let visitors listen without an account. Off by default; guests never reach the device settings, bookmarks, profiles or this console."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {p && (
        <div className="flex flex-col gap-6">
          <Card>
            <div className="flex flex-col gap-4 p-4">
              <Toggle
                label="Allow guest access"
                checked={p.enabled}
                onChange={(enabled) => update({ enabled })}
              />
              <p className="text-xs text-fg-subtle">
                Turning guest access off ends every running guest session and removes their
                receivers.
              </p>
            </div>
          </Card>

          <Card title="Limits" description="Applied per guest and across all guests.">
            <div className="grid gap-4 p-4 sm:grid-cols-2">
              <NumberField
                label="Guests online at once"
                min={0}
                max={1000}
                value={p.maxGuests}
                onCommit={(maxGuests) => update({ maxGuests })}
              />
              <NumberField
                label="Guests per address"
                min={1}
                max={100}
                value={p.maxGuestsPerIp}
                onCommit={(maxGuestsPerIp) => update({ maxGuestsPerIp })}
              />
              <NumberField
                label="Receivers per guest"
                min={1}
                max={16}
                value={p.maxVfosPerGuest}
                onCommit={(maxVfosPerGuest) => update({ maxVfosPerGuest })}
              />
              <NumberField
                label="Idle timeout"
                unit="min"
                min={30}
                max={86_400}
                value={p.idleTimeoutSecs}
                format={(secs) => String(Number((secs / 60).toFixed(1)))}
                parse={(t) => {
                  const n = Number(t.trim().replace(",", "."));
                  return t.trim() === "" || !Number.isFinite(n) ? null : Math.round(n * 60);
                }}
                onCommit={(idleTimeoutSecs) => update({ idleTimeoutSecs })}
              />
            </div>
            <p className="px-4 pb-4 text-xs text-fg-subtle">
              A guest session ends after the idle timeout without requests and without an open
              connection; a visitor who keeps listening stays. Between 0.5 minutes and 24 hours.
            </p>
          </Card>

          <Card
            title="What guests may tune"
            description="Checked on the server for every new receiver."
          >
            <div className="flex flex-col gap-4 p-4">
              <div className="flex flex-col gap-2">
                <span className="text-xs font-medium text-fg-muted">Allowed modes</span>
                <div className="flex flex-wrap gap-2" role="group" aria-label="Allowed modes">
                  {ALL_MODES.map((m) => (
                    <Chip
                      key={m}
                      selected={p.allowedModes.includes(m)}
                      onClick={() => toggleMode(m)}
                    >
                      {MODE_LABEL[m]}
                    </Chip>
                  ))}
                </div>
              </div>
              <Toggle
                label="Limit the frequency range"
                checked={p.freqMinHz !== null || p.freqMaxHz !== null}
                onChange={(on) =>
                  update(
                    on
                      ? {
                          freqMinHz: p.freqMinHz ?? 87_500_000,
                          freqMaxHz: p.freqMaxHz ?? 108_000_000,
                        }
                      : { freqMinHz: null, freqMaxHz: null },
                  )
                }
              />
              {(p.freqMinHz !== null || p.freqMaxHz !== null) && (
                <div className="grid gap-4 sm:grid-cols-2">
                  <NumberField
                    label="Lowest frequency"
                    unit="MHz"
                    min={0}
                    value={p.freqMinHz ?? 0}
                    format={formatMhzText}
                    parse={parseMhzText}
                    onCommit={(freqMinHz) => update({ freqMinHz })}
                  />
                  <NumberField
                    label="Highest frequency"
                    unit="MHz"
                    min={0}
                    value={p.freqMaxHz ?? 0}
                    format={formatMhzText}
                    parse={parseMhzText}
                    onCommit={(freqMaxHz) => update({ freqMaxHz })}
                  />
                </div>
              )}
              <Toggle
                label="Only while the tuning is not locked"
                checked={p.onlyWithoutLock}
                onChange={(onlyWithoutLock) => update({ onlyWithoutLock })}
              />
              <p className="-mt-2 text-xs text-fg-subtle">
                When an operator holds the tuning lock, guests cannot open new receivers.
              </p>
              <Toggle
                label="Show decoder history to guests"
                checked={p.showDecoderEvents}
                onChange={(showDecoderEvents) => update({ showDecoderEvents })}
              />
            </div>
          </Card>

          <Card
            title="Privacy notice"
            description="Shown to every visitor before a guest session starts."
          >
            <div className="p-4">
              <TextArea
                label="Notice text"
                rows={6}
                maxLength={4000}
                value={p.privacyText}
                onChange={(privacyText) => update({ privacyText })}
                hint="Say what is stored (address, browser, actions) and for how long. See Audit settings."
              />
            </div>
          </Card>

          {problem && <Alert tone="warn">{problem}</Alert>}
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
