import { useState } from "react";
import { useAuth } from "../auth/context";
import { roleLabel } from "../auth/permissions";
import { Badge, PlainButton, Popover, cn } from "../ui";
import { OPEN_SESSIONS_EVENT, useAppEvent } from "./appEvents";
import { AccountDialogs, AccountSection, roleTone, type AccountDialogKind } from "./AccountSection";

/** Up to two initials of a login name ("alice.smith" -> "AS", "root" -> "R"). */
export function initials(name: string): string {
  const parts = name
    .trim()
    .split(/[\s._@-]+/)
    .filter(Boolean);
  const first = parts[0] ?? "?";
  const second = parts[1];
  const out = (Array.from(first)[0] ?? "?") + (second ? (Array.from(second)[0] ?? "") : "");
  return out.toUpperCase();
}

/**
 * Always visible account button at the top right: avatar with initials, name and role badge. Opens
 * the account menu (admin console for admins, own sessions, password, sign out). The same entries
 * stay in the settings popover. Renders nothing without a session (tests, no auth gate).
 */
export function AccountMenu() {
  const auth = useAuth();
  const [dialog, setDialog] = useState<AccountDialogKind | null>(null);
  useAppEvent(OPEN_SESSIONS_EVENT, () => {
    if (auth?.permissions.canManageSessions) setDialog("sessions");
  });
  if (!auth) return null;
  const perms = auth.permissions;
  return (
    <>
      <Popover
        label="Account"
        panelClassName="w-64"
        trigger={({ open, props }) => (
          <PlainButton
            {...props}
            title={`${auth.username} (${roleLabel(perms)}): account menu`}
            aria-label={`Account menu, signed in as ${auth.username}`}
            data-testid="account-button"
            className={cn(
              "inline-flex h-8 items-center gap-2 rounded-full border border-border-strong bg-surface-2 py-0 pr-3 pl-0.5 text-sm text-fg transition-colors duration-150 hover:bg-surface-3 max-phone:size-11 max-phone:justify-center max-phone:p-0",
              open && "bg-surface-3",
            )}
          >
            <span
              aria-hidden="true"
              className="grid size-7 shrink-0 place-items-center rounded-full bg-accent-soft text-xs font-semibold text-accent"
            >
              {initials(auth.username)}
            </span>
            <span className="max-w-28 truncate font-medium max-phone:hidden">{auth.username}</span>
            <span className="max-shell:hidden">
              <Badge tone={roleTone(perms)}>{roleLabel(perms)}</Badge>
            </span>
          </PlainButton>
        )}
      >
        {(close) => (
          <div className="flex flex-col gap-2">
            <div className="flex min-w-0 items-center gap-2 px-1">
              <span
                aria-hidden="true"
                className="grid size-9 shrink-0 place-items-center rounded-full bg-accent-soft text-sm font-semibold text-accent"
              >
                {initials(auth.username)}
              </span>
              <div className="min-w-0">
                <div className="truncate text-sm font-medium" data-testid="account-menu-user">
                  {auth.username}
                </div>
                <Badge tone={roleTone(perms)}>{roleLabel(perms)}</Badge>
              </div>
            </div>
            <AccountSection onClose={close} onDialog={setDialog} hideIdentity bordered={false} />
          </div>
        )}
      </Popover>
      <AccountDialogs kind={dialog} onClose={() => setDialog(null)} />
    </>
  );
}
