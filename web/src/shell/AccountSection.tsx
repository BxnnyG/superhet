import { ChangePasswordDialog } from "../auth/ChangePasswordDialog";
import { useAuth } from "../auth/context";
import { roleLabel } from "../auth/permissions";
import { MySessionsDialog } from "../auth/MySessionsDialog";
import { navigate } from "../router";
import { Badge, Button, KeyIcon, LogOutIcon, ShieldIcon, UserIcon, UsersIcon, cn } from "../ui";

/** Badge tone of a role. */
export function roleTone(perms: { isAdmin: boolean; isOperator: boolean }) {
  return perms.isAdmin ? "danger" : perms.isOperator ? "accent" : "neutral";
}

export type AccountDialogKind = "password" | "sessions";

/** The dialogs the account block opens; rendered outside the popover so closing it keeps them. */
export function AccountDialogs({
  kind,
  onClose,
}: {
  kind: AccountDialogKind | null;
  onClose: () => void;
}) {
  const auth = useAuth();
  if (!auth || kind === null) return null;
  return kind === "password" ? (
    <ChangePasswordDialog client={auth.client} onClose={onClose} />
  ) : (
    <MySessionsDialog onClose={onClose} />
  );
}

/**
 * Account block of the settings popover: who is signed in with the role badge, own sessions,
 * password, the admin console (admins) and sign out. Guests have no account page and no password.
 */
export function AccountSection({
  onClose,
  onDialog,
  bordered = true,
  hideIdentity = false,
}: {
  onClose: () => void;
  onDialog: (kind: AccountDialogKind) => void;
  /** Top divider (the settings popover); the account menu has its own header. */
  bordered?: boolean;
  /** Skip the name and role row (shown elsewhere). */
  hideIdentity?: boolean;
}) {
  const auth = useAuth();
  if (!auth) return null;
  const { permissions: perms } = auth;
  return (
    <>
      <div className={cn("flex flex-col gap-1", bordered && "border-t border-border pt-3")}>
        {!hideIdentity && (
          <div className="flex min-w-0 items-center gap-2 px-1 pb-1 text-sm">
            <UserIcon className="shrink-0 text-fg-subtle" />
            <span className="sr-only">Signed in as</span>
            <span className="min-w-0 truncate font-medium" data-testid="signed-in-user">
              {auth.username}
            </span>
            <Badge
              tone={roleTone(perms)}
              title={
                perms.isGuest
                  ? "Guest session: you can listen and open a few receivers"
                  : `Role: ${roleLabel(perms)}`
              }
            >
              {roleLabel(perms)}
            </Badge>
          </div>
        )}
        {perms.canAdmin && (
          <Button
            variant="ghost"
            icon={<ShieldIcon />}
            className="justify-start"
            onClick={() => {
              onClose();
              navigate("/admin");
            }}
          >
            Admin console
          </Button>
        )}
        {perms.canManageSessions && (
          <Button
            variant="ghost"
            icon={<UsersIcon />}
            className="justify-start"
            onClick={() => {
              onClose();
              onDialog("sessions");
            }}
          >
            My sessions
          </Button>
        )}
        {!perms.isGuest && (
          <Button
            variant="ghost"
            icon={<KeyIcon />}
            className="justify-start"
            onClick={() => {
              onClose();
              onDialog("password");
            }}
          >
            Change password
          </Button>
        )}
        <Button
          variant="ghost"
          icon={<LogOutIcon />}
          className="justify-start"
          onClick={() => {
            onClose();
            void auth.signOut();
          }}
        >
          {perms.isGuest ? "End guest session" : "Sign out"}
        </Button>
      </div>
    </>
  );
}
