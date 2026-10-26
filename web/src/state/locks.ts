// Wording for tuning lock holders (docs/api-devices.md, "Tuning lock"; docs/api-users.md: the
// holder of a user lock is an account, `id` = `u:<id>`, `name` = its login).

import type { LockHolder } from "../spectrum/api";

/**
 * Which lock holder record is "me": `u:<id>` for an account, `null` when the session cannot hold a
 * lock (guest), `undefined` when unknown (no sign-in, single-user setups): every user lock counts
 * as the viewer's own then.
 */
export type LockIdentity = string | null | undefined;

let identity: LockIdentity;

/** Set by the auth gate once the session is known. */
export function setLockIdentity(id: LockIdentity): void {
  identity = id;
}

export function isLocked(holders: readonly LockHolder[] | undefined): boolean {
  return (holders?.length ?? 0) > 0;
}

/** The holder is the viewer's own user lock. */
export function isMine(h: LockHolder, me: LockIdentity = identity): boolean {
  if (h.kind !== "user") return false;
  if (me === undefined) return true;
  return me !== null && h.id === me;
}

export function holdsUserLock(
  holders: readonly LockHolder[],
  me: LockIdentity = identity,
): boolean {
  return holders.some((h) => isMine(h, me));
}

/** True when only the viewer's own lock is in the way (so "unlock" can clear it). */
export function onlyUserLock(holders: readonly LockHolder[], me: LockIdentity = identity): boolean {
  return holders.length > 0 && holders.every((h) => isMine(h, me));
}

/** A user lock held by somebody else (an admin can force-release it). */
export function foreignUserLock(
  holders: readonly LockHolder[],
  me: LockIdentity = identity,
): boolean {
  return holders.some((h) => h.kind === "user" && !isMine(h, me));
}

const KIND_LABEL: Record<string, string> = {
  recording: "a recording",
  decoder: "a decoder",
  claim: "an exclusive claim",
};

function holderLabel(h: LockHolder, me: LockIdentity): string {
  if (h.kind === "user") return isMine(h, me) ? "you" : (h.name ?? "another user");
  return KIND_LABEL[h.kind] ?? h.kind;
}

function joinList(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** "you", "you and a recording", "alice", ... (distinct labels, the viewer first). */
export function describeHolders(
  holders: readonly LockHolder[],
  me: LockIdentity = identity,
): string {
  const labels = [...new Set(holders.map((h) => holderLabel(h, me)))].sort(
    (a, b) => Number(b === "you") - Number(a === "you"),
  );
  return joinList(labels);
}

/** "Locked by you" / "Locked by alice" / "Locked by you and a recording". */
export function lockedBy(holders: readonly LockHolder[], me: LockIdentity = identity): string {
  return `Locked by ${describeHolders(holders, me)}`;
}
