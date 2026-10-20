// Client-side password checks (the server enforces the same length rules) and a rough strength hint.

import { PASSWORD_MAX, PASSWORD_MIN } from "./api";

export interface Strength {
  /** 0 (too short / trivial) to 4 (strong). */
  score: 0 | 1 | 2 | 3 | 4;
  label: string;
}

const LABELS = ["Too weak", "Weak", "Fair", "Good", "Strong"] as const;

/** Length plus character-class variety; a hint only, never a gate beyond the minimum length. */
export function passwordStrength(pw: string): Strength {
  if (pw.length === 0) return { score: 0, label: "" };
  if (pw.length < PASSWORD_MIN) return { score: 0, label: LABELS[0] };
  const classes = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^A-Za-z0-9]/].filter((r) => r.test(pw)).length;
  const unique = new Set(pw).size;
  let score = 1;
  if (pw.length >= 16 || classes >= 3) score += 1;
  if (pw.length >= 20 && classes >= 2) score += 1;
  if (pw.length >= 24 && classes >= 3) score += 1;
  if (unique < 6) score = Math.min(score, 1);
  const s = Math.min(4, score) as Strength["score"];
  return { score: s, label: LABELS[s] };
}

export function passwordLengthError(pw: string): string | null {
  if (pw.length < PASSWORD_MIN) return `Use at least ${PASSWORD_MIN} characters.`;
  if (pw.length > PASSWORD_MAX) return `Use at most ${PASSWORD_MAX} characters.`;
  return null;
}
