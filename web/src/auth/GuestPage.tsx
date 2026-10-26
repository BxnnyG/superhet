import { useState } from "react";
import { Alert, Button } from "../ui";
import { AuthError, type AuthClient, type GuestInfo } from "./api";
import { AuthLayout } from "./AuthLayout";

interface Props {
  client: AuthClient;
  guest: GuestInfo;
  /** The guest session exists now; the gate loads the app. */
  onStarted: () => void;
  onBack: () => void;
}

/** What the operator says about guest access, then the button that starts the guest session. */
export function GuestPage({ client, guest, onStarted, onBack }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const start = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await client.guestStart();
      onStarted();
    } catch (err) {
      if (err instanceof AuthError && err.kind === "guests_full") {
        setError("All guest places are taken right now. Please try again later.");
      } else if (err instanceof AuthError && err.kind === "too_many_guests") {
        setError("There are already too many guest sessions from your address.");
      } else if (err instanceof AuthError && err.kind === "not_found") {
        setError("Guest access has just been turned off.");
      } else {
        setError(err instanceof AuthError ? err.message : "Could not start a guest session.");
      }
      setBusy(false);
    }
  };

  return (
    <AuthLayout title="Continue as guest" subtitle="Listen along without an account.">
      <div className="flex flex-col gap-4">
        {error && <Alert>{error}</Alert>}
        {guest.privacyText ? (
          <section
            aria-label="Privacy notice"
            className="max-h-64 overflow-y-auto rounded-lg border border-border bg-surface-2 p-3 text-sm whitespace-pre-wrap text-fg-muted"
          >
            {guest.privacyText}
          </section>
        ) : null}
        <p className="text-xs text-fg-subtle">
          Guests can listen and open a limited number of receivers. They cannot change the device
          settings, and their session ends after a period of inactivity.
        </p>
        <Button variant="primary" size="lg" disabled={busy} onClick={() => void start()}>
          {busy ? "Starting…" : "Accept and continue as guest"}
        </Button>
        <Button variant="ghost" onClick={onBack} disabled={busy}>
          Back to sign in
        </Button>
      </div>
    </AuthLayout>
  );
}
