// Wording for the 403 answers of the multi-user API (docs/api-users.md): the codes the device
// routes put in `code` (`not_owner`, `vfo_limit`, `mode_not_allowed`, `frequency_not_allowed`,
// `locked` for guests) and the access table's plain `forbidden` / `password_change_required`.

import { ApiError } from "../spectrum/api";
import { toasts } from "../ui/toast";

export interface AccessNotice {
  title: string;
  body: string;
}

const NOTICES: Record<string, AccessNotice> = {
  not_owner: {
    title: "Not your receiver",
    body: "This receiver belongs to someone else. Only its owner, an operator or an admin can change it.",
  },
  vfo_limit: {
    title: "Receiver limit reached",
    body: "You cannot open more receivers. Close one first, or ask an admin to raise the limit.",
  },
  mode_not_allowed: {
    title: "Mode not allowed",
    body: "This mode is not open to guests. Pick one of the allowed modes.",
  },
  frequency_not_allowed: {
    title: "Frequency not allowed",
    body: "This frequency is outside the range open to guests.",
  },
  locked: {
    title: "Tuning is locked",
    body: "Guests cannot open receivers while an operator holds the tuning lock.",
  },
  forbidden: {
    title: "Not allowed",
    body: "Your role is not allowed to do that.",
  },
  password_change_required: {
    title: "Change your password first",
    body: "This account has a one-time password. Set a new one to continue.",
  },
};

/** The refusal code of a 403 ApiError, or null for every other error. */
export function accessCode(e: unknown): string | null {
  if (!(e instanceof ApiError) || e.status !== 403) return null;
  return e.code ?? e.message;
}

/** Wording of a 403 answer; null when the error is not one of the known refusals. */
export function accessNotice(e: unknown): AccessNotice | null {
  const code = accessCode(e);
  if (code === null) return null;
  return NOTICES[code] ?? NOTICES.forbidden ?? null;
}

/** Show a 403 refusal as a toast. Returns false when the error is not an access refusal. */
export function reportAccessError(e: unknown): boolean {
  const notice = accessNotice(e);
  if (!notice) return false;
  toasts.push({ key: `access-${notice.title}`, tone: "warn", ...notice });
  return true;
}
