// Form state of the rule editor and its conversion to and from the API documents.

import type { Condition, ConditionOp, Rule, RuleBody } from "./api";

/** The operators the editor offers; "any of" variants take a comma separated list. */
export type UiOp = ConditionOp | "equals_any" | "contains_any";

export const OP_LABELS: Record<UiOp, string> = {
  equals: "equals",
  equals_any: "equals any of (comma separated)",
  not_equals: "does not equal",
  contains: "contains",
  contains_any: "contains any of (comma separated)",
  not_contains: "does not contain",
  regex: "matches regex",
  gt: "is greater than",
  gte: "is at least",
  lt: "is less than",
  lte: "is at most",
  exists: "is present",
};

/** Fields decoders commonly provide; the field input offers them as suggestions. */
export const FIELD_SUGGESTIONS = [
  "value",
  "kind",
  "source",
  "device",
  "freq_hz",
  "data.text",
  "data.address",
  "data.callsign",
  "data.talkgroup",
  "data.icao",
  "data.id",
];

export interface ConditionDraft {
  field: string;
  op: UiOp;
  value: string;
  caseSensitive: boolean;
}

export interface RuleDraft {
  name: string;
  enabled: boolean;
  source: string;
  kind: string;
  conditions: ConditionDraft[];
  cooldownSecs: number;
  maxPerHour: number | null;
  quietOn: boolean;
  quietStart: string;
  quietEnd: string;
  quietTz: string;
  titleTemplate: string;
  bodyTemplate: string;
  targetIds: number[];
}

export function emptyDraft(): RuleDraft {
  return {
    name: "",
    enabled: true,
    source: "",
    kind: "",
    conditions: [],
    cooldownSecs: 0,
    maxPerHour: null,
    quietOn: false,
    quietStart: "22:00",
    quietEnd: "06:00",
    quietTz: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
    titleTemplate: "",
    bodyTemplate: "",
    targetIds: [],
  };
}

export function ruleToDraft(rule: Rule): RuleDraft {
  return {
    name: rule.name,
    enabled: rule.enabled,
    source: rule.source ?? "",
    kind: rule.kind ?? "",
    conditions: rule.conditions.map((c) => {
      const list = Array.isArray(c.value);
      const op: UiOp =
        list && c.op === "equals"
          ? "equals_any"
          : list && c.op === "contains"
            ? "contains_any"
            : c.op;
      const value = Array.isArray(c.value)
        ? c.value.join(", ")
        : c.value === undefined
          ? ""
          : String(c.value);
      return { field: c.field, op, value, caseSensitive: c.case_sensitive === true };
    }),
    cooldownSecs: rule.cooldownSecs,
    maxPerHour: rule.maxPerHour,
    quietOn: rule.quiet !== null,
    quietStart: rule.quiet?.start ?? "22:00",
    quietEnd: rule.quiet?.end ?? "06:00",
    quietTz: rule.quiet?.tz ?? (Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC"),
    titleTemplate: rule.titleTemplate,
    bodyTemplate: rule.bodyTemplate,
    targetIds: [...rule.targetIds],
  };
}

const NUMERIC: readonly UiOp[] = ["gt", "gte", "lt", "lte"];

/** One value as a number when it reads as a plain number (so `1234` matches the RIC 1234). */
function scalar(text: string): string | number {
  return /^-?\d+(\.\d+)?$/.test(text) ? Number(text) : text;
}

function conditionBody(c: ConditionDraft): Condition {
  const base = { field: c.field.trim() };
  const cs = c.caseSensitive ? { case_sensitive: true } : {};
  switch (c.op) {
    case "exists":
      return { ...base, op: "exists" };
    case "equals_any":
    case "contains_any": {
      const items = c.value
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean)
        .map(c.op === "equals_any" ? scalar : (s) => s);
      return { ...base, op: c.op === "equals_any" ? "equals" : "contains", value: items, ...cs };
    }
    case "regex":
      return { ...base, op: "regex", value: c.value, ...cs };
    default:
      if (NUMERIC.includes(c.op))
        return { ...base, op: c.op as ConditionOp, value: Number(c.value) };
      return {
        ...base,
        op: c.op as ConditionOp,
        value: c.op === "equals" || c.op === "not_equals" ? scalar(c.value.trim()) : c.value,
        ...cs,
      };
  }
}

/** First problem the user can fix before sending, or null. The server validates again. */
export function draftProblem(d: RuleDraft): string | null {
  if (!d.name.trim()) return "Give the rule a name.";
  for (const [i, c] of d.conditions.entries()) {
    const n = `Condition ${i + 1}`;
    if (!c.field.trim()) return `${n} needs a field.`;
    if (c.op !== "exists" && !c.value.trim()) return `${n} needs a value.`;
    if (NUMERIC.includes(c.op) && !Number.isFinite(Number(c.value))) return `${n} needs a number.`;
  }
  if (d.targetIds.length === 0) return "Pick at least one target.";
  return null;
}

export function draftToBody(d: RuleDraft): RuleBody {
  return {
    name: d.name.trim(),
    enabled: d.enabled,
    source: d.source.trim() || null,
    kind: d.kind.trim() || null,
    conditions: d.conditions.map(conditionBody),
    cooldown_secs: d.cooldownSecs,
    max_per_hour: d.maxPerHour,
    quiet: d.quietOn ? { start: d.quietStart, end: d.quietEnd, tz: d.quietTz.trim() } : null,
    title_template: d.titleTemplate,
    body_template: d.bodyTemplate,
    target_ids: d.targetIds,
  };
}
