import { useCallback, useState, type SyntheticEvent } from "react";
import { ActionsMenu } from "../bookmarks/ActionsMenu";
import { PASSWORD_MIN, ROLES, USERNAME_RE, type Role } from "../auth/api";
import { ROLE_LABEL } from "../auth/permissions";
import { formatDateTime, formatRelative } from "../format";
import { HttpError, describeHttpError } from "../http";
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  CopyIcon,
  Dialog,
  EmptyState,
  LinkIcon,
  MoreIcon,
  PasswordField,
  PlusIcon,
  Select,
  TextField,
  cn,
  toasts,
  type BadgeTone,
} from "../ui";
import type { AdminApi, Invite, InviteState, User } from "./api";
import { Card, LoadState, RoleBadge, SectionHeader, copyText, useNow } from "./common";
import { useLoad } from "./useLoad";

const ROLE_OPTIONS = ROLES.map((r) => ({ value: r, label: ROLE_LABEL[r] }));

const EXPIRY_OPTIONS = [
  { value: "1", label: "1 hour" },
  { value: "24", label: "1 day" },
  { value: "72", label: "3 days (default)" },
  { value: "168", label: "7 days" },
  { value: "720", label: "30 days" },
];

function fail(title: string, e: unknown) {
  toasts.push({ tone: "danger", title, body: describeHttpError(e), key: title });
}

export function UsersSection({ api, selfId }: { api: AdminApi; selfId: number | null }) {
  const listUsers = useCallback((s: AbortSignal) => api.listUsers(s), [api]);
  const users = useLoad(listUsers, 20_000);
  const listInvites = useCallback((s: AbortSignal) => api.listInvites(s), [api]);
  const invites = useLoad(listInvites, 20_000);

  const [creating, setCreating] = useState(false);
  const [inviting, setInviting] = useState(false);
  const [secret, setSecret] = useState<{ title: string; user: string; password: string } | null>(
    null,
  );
  const [link, setLink] = useState<{ url: string; invite: Invite } | null>(null);
  const [confirm, setConfirm] = useState<
    | { kind: "delete"; user: User }
    | { kind: "disable"; user: User }
    | { kind: "reset"; user: User }
    | { kind: "revoke"; invite: Invite }
    | null
  >(null);
  const [busy, setBusy] = useState(false);

  const list = users.data ?? [];
  const activeAdmins = list.filter((u) => u.role === "admin" && !u.disabled).length;

  const run = async (fn: () => Promise<void>, errTitle: string) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      fail(errTitle, e);
    } finally {
      setBusy(false);
      users.reload();
      invites.reload();
    }
  };

  const changeRole = (u: User, role: Role) =>
    run(async () => {
      await api.updateUser(u.id, { role });
      toasts.push({ tone: "success", title: `${u.username} is now ${ROLE_LABEL[role]}` });
    }, "Could not change the role");

  const setDisabled = (u: User, disabled: boolean) =>
    run(async () => {
      await api.updateUser(u.id, { disabled });
      toasts.push({
        tone: "success",
        title: disabled ? `${u.username} was disabled` : `${u.username} was enabled`,
      });
    }, "Could not update the account");

  return (
    <div>
      <SectionHeader
        title="Users and invitations"
        description="Accounts, their roles and invitation links. Role changes apply to the next request of existing sessions."
        actions={
          <>
            <Button icon={<LinkIcon size={14} />} onClick={() => setInviting(true)}>
              Invite
            </Button>
            <Button
              variant="primary"
              icon={<PlusIcon size={14} />}
              onClick={() => setCreating(true)}
            >
              New user
            </Button>
          </>
        }
      />

      <Card title="Users" description={users.data ? `${list.length} accounts` : undefined}>
        <LoadState loading={users.loading} error={users.error} onRetry={users.reload} />
        {users.data && (
          <ul className="divide-y divide-border" aria-label="Users">
            {list.map((u) => {
              const isSelf = u.id === selfId;
              const lastAdmin = u.role === "admin" && !u.disabled && activeAdmins <= 1;
              return (
                <li
                  key={u.id}
                  className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3"
                  data-testid={`user-${u.username}`}
                >
                  <div className="flex min-w-0 flex-1 basis-56 items-center gap-3">
                    <span
                      aria-hidden="true"
                      className="grid size-8 shrink-0 place-items-center rounded-full bg-surface-2 text-xs font-semibold text-fg-muted uppercase"
                    >
                      {u.username.slice(0, 2)}
                    </span>
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="truncate text-sm font-medium">{u.username}</span>
                        {isSelf && <Badge tone="accent">You</Badge>}
                        {u.disabled && <Badge tone="danger">Disabled</Badge>}
                        {u.mustChangePassword && (
                          <Badge tone="warn" title="Signs in with a one-time password">
                            One-time password
                          </Badge>
                        )}
                      </div>
                      <p className="text-xs text-fg-muted">
                        {u.lastLoginMs
                          ? `Last sign-in ${formatRelative(u.lastLoginMs)}`
                          : "Never signed in"}
                        {" · "}
                        {u.sessions} {u.sessions === 1 ? "session" : "sessions"}
                      </p>
                    </div>
                  </div>
                  <Select
                    aria-label={`Role of ${u.username}`}
                    size="sm"
                    className="w-32"
                    value={u.role}
                    disabled={busy || lastAdmin}
                    title={lastAdmin ? "The last active admin cannot be demoted" : undefined}
                    onChange={(role) => void changeRole(u, role as Role)}
                    options={ROLE_OPTIONS}
                  />
                  <ActionsMenu
                    label={`Actions for ${u.username}`}
                    icon={<MoreIcon />}
                    actions={[
                      u.disabled
                        ? { label: "Enable account", onSelect: () => void setDisabled(u, false) }
                        : {
                            label: "Disable account…",
                            disabled: isSelf || lastAdmin,
                            onSelect: () => setConfirm({ kind: "disable", user: u }),
                          },
                      {
                        label: "Reset password…",
                        onSelect: () => setConfirm({ kind: "reset", user: u }),
                      },
                      {
                        label: "Delete account…",
                        separated: true,
                        disabled: isSelf || lastAdmin,
                        onSelect: () => setConfirm({ kind: "delete", user: u }),
                      },
                    ]}
                  />
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <Card
        className="mt-6"
        title="Invitations"
        description="A link lets one person create an account with the role fixed here. It works once."
      >
        <LoadState loading={invites.loading} error={invites.error} onRetry={invites.reload} />
        {invites.data && invites.data.length === 0 && (
          <div className="p-4">
            <EmptyState title="No invitations yet">
              Create one to add a user without choosing their password.
            </EmptyState>
          </div>
        )}
        {invites.data && invites.data.length > 0 && (
          <InviteList
            invites={invites.data}
            onRevoke={(invite) => setConfirm({ kind: "revoke", invite })}
          />
        )}
      </Card>

      {creating && (
        <NewUserDialog
          api={api}
          onClose={() => setCreating(false)}
          onCreated={(user, password) => {
            setCreating(false);
            users.reload();
            if (password) {
              setSecret({ title: "One-time password", user: user.username, password });
            } else {
              toasts.push({ tone: "success", title: `Account ${user.username} created` });
            }
          }}
        />
      )}
      {inviting && (
        <NewInviteDialog
          api={api}
          onClose={() => setInviting(false)}
          onCreated={(invite, path) => {
            setInviting(false);
            invites.reload();
            setLink({ url: `${window.location.origin}${path}`, invite });
          }}
        />
      )}
      {secret && (
        <SecretDialog
          title={secret.title}
          user={secret.user}
          password={secret.password}
          onClose={() => setSecret(null)}
        />
      )}
      {link && (
        <InviteLinkDialog url={link.url} invite={link.invite} onClose={() => setLink(null)} />
      )}

      {confirm?.kind === "delete" && (
        <ConfirmDialog
          title={`Delete ${confirm.user.username}?`}
          confirmLabel="Delete account"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            const u = confirm.user;
            setConfirm(null);
            void run(async () => {
              await api.deleteUser(u.id);
              toasts.push({ tone: "success", title: `${u.username} was deleted` });
            }, "Could not delete the account");
          }}
        >
          The account and all its sessions are removed. Audit entries stay, with the name replaced
          by a placeholder. This cannot be undone.
        </ConfirmDialog>
      )}
      {confirm?.kind === "disable" && (
        <ConfirmDialog
          title={`Disable ${confirm.user.username}?`}
          confirmLabel="Disable account"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            const u = confirm.user;
            setConfirm(null);
            void setDisabled(u, true);
          }}
        >
          The account is signed out everywhere at once and cannot sign in until you enable it.
        </ConfirmDialog>
      )}
      {confirm?.kind === "revoke" && (
        <ConfirmDialog
          title="Revoke this invitation?"
          confirmLabel="Revoke"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setConfirm(null)}
          onConfirm={() => {
            const inv = confirm.invite;
            setConfirm(null);
            void run(async () => {
              await api.revokeInvite(inv.id);
              toasts.push({ tone: "success", title: "Invitation revoked" });
            }, "Could not revoke the invitation");
          }}
        >
          The link stops working immediately.
        </ConfirmDialog>
      )}
      {confirm?.kind === "reset" && (
        <ResetPasswordDialog
          user={confirm.user}
          api={api}
          onClose={() => setConfirm(null)}
          onDone={(password) => {
            const u = confirm.user;
            setConfirm(null);
            users.reload();
            if (password) setSecret({ title: "New one-time password", user: u.username, password });
            else toasts.push({ tone: "success", title: `Password of ${u.username} was reset` });
          }}
        />
      )}
    </div>
  );
}

const STATE_TONE: Record<InviteState, BadgeTone> = {
  open: "success",
  used: "neutral",
  expired: "warn",
  revoked: "danger",
};

function InviteList({
  invites,
  onRevoke,
}: {
  invites: readonly Invite[];
  onRevoke: (i: Invite) => void;
}) {
  const now = useNow();
  return (
    <ul className="divide-y divide-border" aria-label="Invitations">
      {invites.map((i) => (
        <li key={i.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
          <div className="flex min-w-0 flex-1 basis-56 flex-col gap-1">
            <div className="flex flex-wrap items-center gap-1.5">
              <RoleBadge role={i.role} />
              <Badge tone={STATE_TONE[i.state]}>{i.state}</Badge>
              {i.note && <span className="truncate text-sm">{i.note}</span>}
            </div>
            <p className="text-xs text-fg-muted">
              Created {formatRelative(i.createdAtMs, now)}
              {i.createdBy ? ` by ${i.createdBy}` : ""}
              {i.state === "open" && (
                <>
                  {" · "}expires {formatRelative(i.expiresAtMs, now)} (
                  {formatDateTime(i.expiresAtMs)})
                </>
              )}
              {i.state === "used" && i.usedBy ? ` · used by ${i.usedBy}` : ""}
            </p>
          </div>
          {i.state === "open" && (
            <Button size="sm" variant="danger" onClick={() => onRevoke(i)}>
              Revoke
            </Button>
          )}
        </li>
      ))}
    </ul>
  );
}

export function validateNewUser(v: { username: string; password: string }) {
  const e: { username?: string; password?: string } = {};
  if (!USERNAME_RE.test(v.username)) {
    e.username = "1 to 64 characters: letters, digits, dot, underscore or hyphen.";
  }
  if (v.password !== "" && v.password.length < PASSWORD_MIN) {
    e.password = `Use at least ${PASSWORD_MIN} characters, or leave empty to generate one.`;
  }
  return e;
}

function NewUserDialog({
  api,
  onClose,
  onCreated,
}: {
  api: AdminApi;
  onClose: () => void;
  onCreated: (user: User, password: string | null) => void;
}) {
  const [username, setUsername] = useState("");
  const [role, setRole] = useState<Role>("listener");
  const [password, setPassword] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nameError, setNameError] = useState<string | null>(null);
  const errors = submitted ? validateNewUser({ username, password }) : {};

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setError(null);
    setNameError(null);
    if (Object.keys(validateNewUser({ username, password })).length > 0) return;
    setBusy(true);
    try {
      const r = await api.createUser({ username, role, ...(password ? { password } : {}) });
      // A password chosen here is not shown again; only a generated one is.
      onCreated(r.user, r.password);
    } catch (err) {
      const msg = describeHttpError(err);
      if (err instanceof HttpError && err.code === "username_taken") setNameError(msg);
      else setError(msg);
      setBusy(false);
    }
  };

  return (
    <Dialog title="New user" onClose={onClose}>
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {error && <Alert>{error}</Alert>}
        <TextField
          label="Username"
          name="username"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          maxLength={64}
          value={username}
          error={errors.username ?? nameError}
          onChange={(e) => setUsername(e.target.value)}
        />
        <Select
          label="Role"
          value={role}
          onChange={(v) => setRole(v as Role)}
          options={ROLE_OPTIONS}
        />
        <RoleHelp role={role} />
        <PasswordField
          label="Password (optional)"
          name="new-password"
          autoComplete="new-password"
          value={password}
          error={errors.password}
          hint="Leave empty to generate a one-time password. The user must replace it at first sign-in."
          onChange={(e) => setPassword(e.target.value)}
        />
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? "Creating…" : "Create user"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

const ROLE_HELP: Record<Role, string> = {
  admin: "Everything, including this console.",
  operator: "Tunes the device, takes the tuning lock, manages profiles and bookmarks.",
  listener: "Listens, opens own receivers (within the limits), reads bookmarks.",
};

function RoleHelp({ role }: { role: Role }) {
  return <p className="-mt-2 text-xs text-fg-subtle">{ROLE_HELP[role]}</p>;
}

function NewInviteDialog({
  api,
  onClose,
  onCreated,
}: {
  api: AdminApi;
  onClose: () => void;
  onCreated: (invite: Invite, path: string) => void;
}) {
  const [role, setRole] = useState<Role>("listener");
  const [hours, setHours] = useState("72");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.createInvite({
        role,
        expiresInHours: Number(hours),
        ...(note.trim() ? { note: note.trim() } : {}),
      });
      onCreated(r.invite, r.path);
    } catch (err) {
      setError(describeHttpError(err));
      setBusy(false);
    }
  };

  return (
    <Dialog title="Invite someone" onClose={onClose}>
      <form onSubmit={submit} className="flex flex-col gap-4">
        {error && <Alert>{error}</Alert>}
        <Select
          label="Role"
          value={role}
          onChange={(v) => setRole(v as Role)}
          options={ROLE_OPTIONS}
        />
        <RoleHelp role={role} />
        <Select label="Expires after" value={hours} onChange={setHours} options={EXPIRY_OPTIONS} />
        <TextField
          label="Note (optional)"
          name="note"
          maxLength={200}
          hint="Only you see it, for example who the link is for."
          value={note}
          onChange={(e) => setNote(e.target.value)}
        />
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? "Creating…" : "Create link"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function InviteLinkDialog({
  url,
  invite,
  onClose,
}: {
  url: string;
  invite: Invite;
  onClose: () => void;
}) {
  return (
    <Dialog title="Invitation link" onClose={onClose}>
      <Alert tone="warn">
        Copy the link now. It is shown only once and cannot be recovered; create a new invitation if
        you lose it.
      </Alert>
      <div className="flex flex-col gap-1">
        <label htmlFor="invite-url" className="text-xs font-medium text-fg-muted">
          Link for a new {ROLE_LABEL[invite.role]}
        </label>
        <div className="flex gap-2">
          <input
            id="invite-url"
            readOnly
            value={url}
            onFocus={(e) => e.currentTarget.select()}
            className="focus-ring h-9 min-w-0 flex-1 rounded-md border border-border-strong bg-surface-2 px-2.5 font-mono text-xs text-fg"
          />
          <Button icon={<CopyIcon size={14} />} onClick={() => void copyText(url, "Link")}>
            Copy
          </Button>
        </div>
        <p className="text-xs text-fg-subtle">
          Expires {formatRelative(invite.expiresAtMs)} ({formatDateTime(invite.expiresAtMs)}). It
          works once.
        </p>
      </div>
      <div className="flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Dialog>
  );
}

function SecretDialog({
  title,
  user,
  password,
  onClose,
}: {
  title: string;
  user: string;
  password: string;
  onClose: () => void;
}) {
  return (
    <Dialog title={title} onClose={onClose}>
      <p className="text-sm text-fg-muted">
        Give this password to <strong className="text-fg">{user}</strong>. It is shown only once;{" "}
        {user} has to choose a new one when signing in.
      </p>
      <div className="flex gap-2">
        <input
          readOnly
          aria-label="One-time password"
          value={password}
          onFocus={(e) => e.currentTarget.select()}
          className={cn(
            "focus-ring h-9 min-w-0 flex-1 rounded-md border border-border-strong",
            "bg-surface-2 px-2.5 font-mono text-sm text-fg",
          )}
        />
        <Button icon={<CopyIcon size={14} />} onClick={() => void copyText(password, "Password")}>
          Copy
        </Button>
      </div>
      <div className="flex justify-end">
        <Button variant="primary" onClick={onClose}>
          Done
        </Button>
      </div>
    </Dialog>
  );
}

function ResetPasswordDialog({
  user,
  api,
  onClose,
  onDone,
}: {
  user: User;
  api: AdminApi;
  onClose: () => void;
  onDone: (generated: string | null) => void;
}) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const invalid = password !== "" && password.length < PASSWORD_MIN;

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy || invalid) return;
    setBusy(true);
    setError(null);
    try {
      onDone(await api.resetPassword(user.id, password || undefined));
    } catch (err) {
      setError(describeHttpError(err));
      setBusy(false);
    }
  };

  return (
    <Dialog title={`Reset password of ${user.username}`} onClose={onClose}>
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        <p className="text-sm text-fg-muted">
          All sessions of {user.username} end at once. The new password is a one-time password: it
          has to be replaced at the next sign-in.
        </p>
        {error && <Alert>{error}</Alert>}
        <PasswordField
          label="Password (optional)"
          name="reset-password"
          autoComplete="new-password"
          value={password}
          error={invalid ? `Use at least ${PASSWORD_MIN} characters.` : null}
          hint="Leave empty to generate one."
          onChange={(e) => setPassword(e.target.value)}
        />
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={busy || invalid}>
            {busy ? "Resetting…" : "Reset password"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
