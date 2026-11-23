import { useCallback, useState } from "react";
import type { AdsbApi } from "../adsb/api";
import { sourceOn, type OnlineAdmin, type OnlineSettings } from "../adsb/online";
import { forgetExtras } from "../adsb/useExtras";
import { describeHttpError } from "../http";
import { Alert, Badge, Toggle, toasts } from "../ui";
import { Card, LoadState } from "./common";
import { useLoad } from "./useLoad";

/**
 * Online aircraft details (ADR-0033): a photo, the route and the full type name from two public
 * services. Off until an admin switches them on here; each service has its own switch, and
 * `SUPERHET_ADSB_ONLINE` on the server can override both (then nothing here is editable).
 */
export function OnlineDetailsCard({ api }: { api: AdsbApi }) {
  const load = useCallback((s: AbortSignal) => api.onlineSettings(s), [api]);
  const loaded = useLoad(load);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const admin: OnlineAdmin | null = loaded.data;

  const save = async (next: OnlineSettings) => {
    setBusy(true);
    setError(null);
    try {
      await api.setOnlineSettings(next);
      forgetExtras(api);
      toasts.push({ tone: "success", title: "Online details saved" });
      loaded.reload();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title="Online details"
      description="A photo, the route and the full type name for the selected aircraft, from public services. Off by default."
    >
      <div className="flex flex-col gap-3 p-4 text-sm" data-testid="online-details">
        <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
        {error !== null && <Alert tone="danger">{error}</Alert>}
        {admin !== null && (
          <>
            <p className="text-fg-muted">
              Switching a service on means: whenever somebody selects an aircraft in the ADS-B view,
              this server asks that service about it. The service learns the aircraft&apos;s ICAO
              address (for the route also its callsign) and the address of this server. The receiver
              position, accounts and viewers are never sent. Without a database the panel still
              works with what the receiver itself hears.
            </p>
            {admin.locked && (
              <Alert tone="info">
                The server sets <code className="font-mono">{admin.env}</code>, which overrides
                these switches: adsbdb{" "}
                {admin.effective.adsbdb && admin.effective.enabled ? "on" : "off"}, Planespotters{" "}
                {admin.effective.planespotters && admin.effective.enabled ? "on" : "off"}. Unset it
                to control them here.
              </Alert>
            )}
            <div className="flex flex-col gap-3">
              <Toggle
                label="Online details"
                checked={admin.settings.enabled}
                disabled={busy || admin.locked}
                onChange={(enabled) =>
                  void save({
                    ...admin.settings,
                    enabled,
                    // Switching on with nothing selected would do nothing: offer both.
                    ...(enabled && !admin.settings.adsbdb && !admin.settings.planespotters
                      ? { adsbdb: true, planespotters: true }
                      : {}),
                  })
                }
              />
              <ul className="flex flex-col gap-3">
                {admin.sources.map((s) => {
                  const id = s.id === "planespotters" ? "planespotters" : "adsbdb";
                  return (
                    <li
                      key={s.id}
                      data-testid={`online-source-${s.id}`}
                      className="rounded-md border border-border p-3"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <a
                          href={s.url}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="font-medium text-accent hover:underline"
                        >
                          {s.name}
                        </a>
                        <div className="flex items-center gap-2">
                          {sourceOn(admin.effective, id) && <Badge tone="success">on</Badge>}
                          <Toggle
                            label={`${s.name}`}
                            hideLabel
                            checked={admin.settings[id]}
                            disabled={busy || admin.locked}
                            onChange={(on) => void save({ ...admin.settings, [id]: on })}
                          />
                        </div>
                      </div>
                      <p className="mt-1 text-xs text-fg-muted">Provides {s.provides}.</p>
                      <p className="text-xs text-fg-muted">Sends {s.sends}.</p>
                    </li>
                  );
                })}
              </ul>
            </div>
            <p className="text-xs text-fg-muted">
              Photos are never copied: each viewer&apos;s browser loads the image straight from
              Planespotters&apos; image servers (the page allows exactly those two hosts only while
              the photo service is on), with the photographer credited and the photo linked, as its
              terms ask. Route data is kept in memory for a few hours and never stored. All of it
              can be wrong or out of date.
            </p>
          </>
        )}
      </div>
    </Card>
  );
}
