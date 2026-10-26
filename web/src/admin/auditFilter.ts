// The audit log filter form and its translation to the query of `GET /api/v1/admin/audit`.

import { ROLES, type Role } from "../auth/api";
import type { ActorKind, AuditFilter, Outcome } from "./api";

export type RangePreset = "1h" | "24h" | "7d" | "30d" | "all" | "custom";

export interface FilterForm {
  range: RangePreset;
  /** `datetime-local` values (local time) for the custom range. */
  from: string;
  to: string;
  actorKind: "" | ActorKind;
  actor: string;
  role: "" | Role;
  action: string;
  outcome: "" | Outcome;
  ip: string;
  target: string;
}

export const EMPTY_FILTER: FilterForm = {
  range: "24h",
  from: "",
  to: "",
  actorKind: "",
  actor: "",
  role: "",
  action: "",
  outcome: "",
  ip: "",
  target: "",
};

const HOUR_MS = 3_600_000;
const PRESET_MS: Record<Exclude<RangePreset, "all" | "custom">, number> = {
  "1h": HOUR_MS,
  "24h": 24 * HOUR_MS,
  "7d": 7 * 24 * HOUR_MS,
  "30d": 30 * 24 * HOUR_MS,
};

export const RANGE_OPTIONS: readonly { value: RangePreset; label: string }[] = [
  { value: "1h", label: "Last hour" },
  { value: "24h", label: "Last 24 hours" },
  { value: "7d", label: "Last 7 days" },
  { value: "30d", label: "Last 30 days" },
  { value: "all", label: "All time" },
  { value: "custom", label: "Custom range…" },
];

/** Milliseconds of a `datetime-local` value (local time); null for empty or invalid input. */
export function parseLocalDateTime(v: string): number | null {
  if (v.trim() === "") return null;
  const ms = new Date(v).getTime();
  return Number.isFinite(ms) ? ms : null;
}

/** The query filter for the form; `now` makes the relative presets testable. */
export function toFilter(f: FilterForm, now: number = Date.now()): AuditFilter {
  const out: AuditFilter = {};
  if (f.range === "custom") {
    const from = parseLocalDateTime(f.from);
    const to = parseLocalDateTime(f.to);
    if (from !== null) out.fromMs = from;
    if (to !== null) out.toMs = to;
  } else if (f.range !== "all") {
    out.fromMs = now - PRESET_MS[f.range];
  }
  if (f.actorKind) out.actorKind = f.actorKind;
  if (f.actor.trim()) out.actor = f.actor.trim();
  if (f.role && ROLES.includes(f.role)) out.role = f.role;
  if (f.action.trim()) out.action = f.action.trim();
  if (f.outcome) out.outcome = f.outcome;
  if (f.ip.trim()) out.ip = f.ip.trim();
  if (f.target.trim()) out.target = f.target.trim();
  return out;
}

/** Why the form cannot be applied (an inverted custom range), or null. */
export function filterProblem(f: FilterForm): string | null {
  if (f.range !== "custom") return null;
  const from = parseLocalDateTime(f.from);
  const to = parseLocalDateTime(f.to);
  if (from !== null && to !== null && from >= to) return "The start must be before the end.";
  return null;
}

/** Number of narrowing fields in use (for the "Filters (3)" hint). */
export function activeFilters(f: FilterForm): number {
  return [
    f.range !== "all",
    f.actorKind,
    f.actor.trim(),
    f.role,
    f.action.trim(),
    f.outcome,
    f.ip.trim(),
    f.target.trim(),
  ].filter(Boolean).length;
}
