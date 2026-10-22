// Wording for tuning lock holders (docs/api-devices.md, "Tuning lock").

import type { LockHolder } from "../spectrum/api";

export function isLocked(holders: readonly LockHolder[] | undefined): boolean {
  return (holders?.length ?? 0) > 0;
}

export function holdsUserLock(holders: readonly LockHolder[]): boolean {
  return holders.some((h) => h.kind === "user");
}

/** True when only the user's own lock is in the way (so "unlock" can clear it). */
export function onlyUserLock(holders: readonly LockHolder[]): boolean {
  return holders.length > 0 && holders.every((h) => h.kind === "user");
}

const KIND_LABEL: Record<string, string> = {
  user: "you",
  recording: "a recording",
  decoder: "a decoder",
  claim: "an exclusive claim",
};

function holderLabel(kind: string): string {
  return KIND_LABEL[kind] ?? kind;
}

function joinList(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** "you", "you and a recording", ... (distinct kinds, user first). */
export function describeHolders(holders: readonly LockHolder[]): string {
  const kinds = [...new Set(holders.map((h) => h.kind))].sort(
    (a, b) => Number(b === "user") - Number(a === "user"),
  );
  return joinList(kinds.map(holderLabel));
}

/** "Locked by you" / "Locked by you and a recording". */
export function lockedBy(holders: readonly LockHolder[]): string {
  return `Locked by ${describeHolders(holders)}`;
}
