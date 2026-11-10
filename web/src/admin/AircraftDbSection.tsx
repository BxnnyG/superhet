import { useCallback, useState } from "react";
import type { AdsbApi } from "../adsb/api";
import type { DatabaseStatus } from "../adsb/info";
import { describeHttpError } from "../http";
import { Alert, Badge, Button, ConfirmDialog, toasts } from "../ui";
import { Card, LoadState, SectionHeader } from "./common";
import { useLoad } from "./useLoad";

const BUSY = new Set(["downloading", "importing"]);

/**
 * The ADS-B aircraft database: where the data comes from, its licence, and the buttons to
 * download, re-import or remove it. Superhet ships no aircraft data.
 */
export function AircraftDbSection({ api }: { api: AdsbApi }) {
  const [busyState, setBusyState] = useState(false);
  const load = useCallback((s: AbortSignal) => api.database(s), [api]);
  // The status is a cheap local query: follow an import while it runs.
  const loaded = useLoad(load, 3000);
  const status: DatabaseStatus | null = loaded.data;
  const running = status !== null && BUSY.has(status.job.state);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);

  const act = async (what: () => Promise<void>, done: string) => {
    setBusyState(true);
    setError(null);
    try {
      await what();
      if (done !== "") toasts.push({ tone: "success", title: done });
      loaded.reload();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusyState(false);
    }
  };

  const offer = status?.offer;
  const db = status?.database ?? null;
  const job = status?.job;

  return (
    <div>
      <SectionHeader
        title="ADS-B data"
        description="Registration, aircraft type and airline names for the ADS-B map and table. Superhet ships no aircraft data: you download it here (or import a copy) and lookups are then local queries on your server."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {error !== null && (
        <Alert tone="danger" className="mb-4">
          {error}
        </Alert>
      )}
      {status !== null && offer !== undefined && job !== undefined && (
        <div className="flex flex-col gap-6">
          <Card
            title="Aircraft database"
            description="Used by everybody who sees decoders; only admins can change it."
          >
            <div className="flex flex-col gap-3 p-4 text-sm">
              {db === null ? (
                <p data-testid="aircraft-db-empty" className="text-fg-muted">
                  No database imported. The ADS-B table shows callsigns and ICAO addresses only.
                </p>
              ) : (
                <dl data-testid="aircraft-db-info" className="grid gap-x-6 gap-y-2 sm:grid-cols-2">
                  <Item k="Source">
                    <a
                      href={db.sourceUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-accent hover:underline"
                    >
                      {db.source}
                    </a>
                  </Item>
                  <Item k="Licence">
                    <a
                      href={db.licenceUrl}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="text-accent hover:underline"
                    >
                      {db.licence}
                    </a>
                  </Item>
                  <Item k="Content">
                    {db.aircraft.toLocaleString("en-US")} aircraft,{" "}
                    {db.types.toLocaleString("en-US")} types, {db.operators.toLocaleString("en-US")}{" "}
                    airlines
                  </Item>
                  <Item k="Version">{db.version === "" ? "unknown" : db.version}</Item>
                  <Item k="Imported">
                    {new Date(db.importedMs).toLocaleString()} (
                    {db.origin === "download" ? "downloaded" : "from a file"})
                  </Item>
                </dl>
              )}
              {job.state === "failed" && (
                <Alert tone="danger">Last import failed: {job.message}</Alert>
              )}
              {job.state === "done" && job.message !== "" && (
                <p className="text-xs text-fg-muted">Last import: {job.message}.</p>
              )}
              {running && (
                <p role="status" className="flex items-center gap-2">
                  <Badge tone="accent">{job.state}</Badge>
                  {job.state === "downloading"
                    ? "Downloading the database…"
                    : "Importing, this takes a few seconds…"}
                </p>
              )}
              <div className="flex flex-wrap gap-2">
                <Button
                  variant="primary"
                  disabled={running || busyState}
                  onClick={() => void act(() => api.updateDatabase("download"), "Download started")}
                >
                  {db === null ? "Download aircraft database" : "Download / update"}
                </Button>
                <Button
                  variant="secondary"
                  disabled={running || (offer.file === null && !offer.archive)}
                  title={
                    offer.file === null && !offer.archive
                      ? "Set SUPERHET_AIRCRAFT_DB_FILE or download first"
                      : undefined
                  }
                  onClick={() => void act(() => api.updateDatabase("file"), "Import started")}
                >
                  Import from file
                </Button>
                {db !== null && (
                  <Button variant="danger" disabled={running} onClick={() => setRemoving(true)}>
                    Remove
                  </Button>
                )}
              </div>
            </div>
          </Card>
          <Card title="Where the data comes from" description="Shown before anything is fetched.">
            <div className="flex flex-col gap-2 p-4 text-sm text-fg-muted">
              <p>
                <strong className="text-fg">{offer.source}</strong>, licence{" "}
                <a
                  href={offer.licenceUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent hover:underline"
                >
                  {offer.licence}
                </a>{" "}
                (Open Data Commons Attribution License: free to use and share if the source is
                credited, which the ADS-B view does). Project page:{" "}
                <a
                  href={offer.sourceUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent hover:underline"
                >
                  {offer.sourceUrl}
                </a>
                .
              </p>
              <p>
                <strong className="text-fg">Download</strong> fetches one file,{" "}
                <code className="font-mono break-all">{offer.downloadUrl}</code>, from your server
                (not from the viewers&apos; browsers) and keeps it under the data directory. Nothing
                is asked from a third party per aircraft.
              </p>
              <p>
                <strong className="text-fg">Offline:</strong> set{" "}
                <code className="font-mono">SUPERHET_AIRCRAFT_DB_FILE</code> on the API to a copy of
                that export (the zip, or a folder with its files) and press Import from file.
                {offer.file !== null && (
                  <>
                    {" "}
                    Currently set to <code className="font-mono break-all">{offer.file}</code>.
                  </>
                )}
                {offer.file === null &&
                  offer.archive &&
                  " The archive from the last download is used."}
              </p>
              <p>
                Airline names come from the callsign (the first three letters, for example DLH for
                Lufthansa), not from the aircraft, so charter and wet-lease flights show the
                operator of the flight number.
              </p>
            </div>
          </Card>
        </div>
      )}
      {removing && (
        <ConfirmDialog
          title="Remove the aircraft database?"
          confirmLabel="Remove"
          confirmVariant="danger"
          busy={busyState}
          onConfirm={() => {
            setRemoving(false);
            void act(() => api.removeDatabase(), "Aircraft database removed");
          }}
          onCancel={() => setRemoving(false)}
        >
          Registration, type and airline names disappear from the ADS-B view until you import a
          database again.
        </ConfirmDialog>
      )}
    </div>
  );
}

function Item({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div>
      <dt className="text-xs text-fg-muted">{k}</dt>
      <dd className="font-medium">{children}</dd>
    </div>
  );
}
