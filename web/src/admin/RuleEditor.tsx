import { useState } from "react";
import { describeHttpError } from "../http";
import type { NotificationsApi, Rule, Target } from "../notifications/api";
import {
  FIELD_SUGGESTIONS,
  OP_LABELS,
  draftProblem,
  draftToBody,
  emptyDraft,
  ruleToDraft,
  type ConditionDraft,
  type RuleDraft,
  type UiOp,
} from "../notifications/ruleDraft";
import {
  Alert,
  Button,
  Dialog,
  IconButton,
  NumberField,
  PlusIcon,
  Select,
  TextField,
  Toggle,
  TrashIcon,
  toasts,
} from "../ui";
import { TextArea } from "./common";

const OPS = Object.keys(OP_LABELS) as UiOp[];
const OP_OPTIONS = OPS.map((value) => ({ value, label: OP_LABELS[value] }));

/**
 * Create or edit a notification rule: which events (source, type, field conditions), how often
 * (cooldown, hourly cap, quiet hours), what the message says and where it goes.
 */
export function RuleEditor({
  rule,
  targets,
  api,
  onClose,
  onSaved,
}: {
  rule: Rule | null;
  targets: Target[];
  api: NotificationsApi;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState<RuleDraft>(() => (rule ? ruleToDraft(rule) : emptyDraft()));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (patch: Partial<RuleDraft>) => setDraft((d) => ({ ...d, ...patch }));
  const setCondition = (i: number, patch: Partial<ConditionDraft>) =>
    set({ conditions: draft.conditions.map((c, j) => (j === i ? { ...c, ...patch } : c)) });

  const save = async () => {
    const problem = draftProblem(draft);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body = draftToBody(draft);
      if (rule) await api.updateRule(rule.id, body);
      else await api.createRule(body);
      toasts.push({ tone: "success", title: rule ? "Rule saved" : "Rule created" });
      onSaved();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={rule ? "Edit rule" : "New rule"} onClose={onClose} size="lg">
      <div className="flex flex-col gap-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <TextField
            label="Name"
            value={draft.name}
            maxLength={80}
            onChange={(e) => set({ name: e.target.value })}
          />
          <div className="flex items-end pb-1">
            <Toggle
              label="Enabled"
              checked={draft.enabled}
              onChange={(enabled) => set({ enabled })}
            />
          </div>
          <TextField
            label="Source (plugin id or core)"
            value={draft.source}
            placeholder="any"
            onChange={(e) => set({ source: e.target.value })}
            hint="For example org.superhet.decoder.pocsag."
          />
          <TextField
            label="Event type"
            value={draft.kind}
            placeholder="any"
            onChange={(e) => set({ kind: e.target.value })}
            hint="For example pocsag.message or dtmf."
          />
        </div>

        <fieldset className="flex flex-col gap-2">
          <legend className="text-xs font-medium text-fg-muted">Conditions (all must match)</legend>
          <datalist id="rule-fields">
            {FIELD_SUGGESTIONS.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
          {draft.conditions.length === 0 && (
            <p className="text-xs text-fg-subtle">
              No conditions: every event of that source and type matches.
            </p>
          )}
          {draft.conditions.map((c, i) => (
            <div
              key={i}
              className="flex flex-wrap items-end gap-2 rounded-lg border border-border p-2"
            >
              <TextField
                className="w-40"
                label="Field"
                list="rule-fields"
                value={c.field}
                onChange={(e) => setCondition(i, { field: e.target.value })}
              />
              <Select
                label="Comparison"
                options={OP_OPTIONS}
                value={c.op}
                onChange={(op) => setCondition(i, { op: op as UiOp })}
              />
              {c.op !== "exists" && (
                <TextField
                  className="min-w-40 flex-1"
                  label="Value"
                  value={c.value}
                  onChange={(e) => setCondition(i, { value: e.target.value })}
                />
              )}
              {c.op !== "exists" && !["gt", "gte", "lt", "lte"].includes(c.op) && (
                <Toggle
                  label="Match case"
                  checked={c.caseSensitive}
                  onChange={(caseSensitive) => setCondition(i, { caseSensitive })}
                />
              )}
              <IconButton
                label={`Remove condition ${i + 1}`}
                size="sm"
                onClick={() => set({ conditions: draft.conditions.filter((_, j) => j !== i) })}
              >
                <TrashIcon />
              </IconButton>
            </div>
          ))}
          <div>
            <Button
              size="sm"
              icon={<PlusIcon size={14} />}
              onClick={() =>
                set({
                  conditions: [
                    ...draft.conditions,
                    { field: "value", op: "contains", value: "", caseSensitive: false },
                  ],
                })
              }
            >
              Add condition
            </Button>
          </div>
        </fieldset>

        <div className="grid gap-3 sm:grid-cols-2">
          <NumberField
            label="Cooldown"
            unit="s"
            min={0}
            max={604800}
            value={draft.cooldownSecs}
            onCommit={(cooldownSecs) => set({ cooldownSecs })}
          />
          <NumberField
            label="At most per hour (0 = no cap)"
            min={0}
            max={100000}
            value={draft.maxPerHour ?? 0}
            onCommit={(n) => set({ maxPerHour: n > 0 ? n : null })}
          />
        </div>

        <div className="flex flex-col gap-2">
          <Toggle
            label="Quiet hours"
            checked={draft.quietOn}
            onChange={(quietOn) => set({ quietOn })}
          />
          {draft.quietOn && (
            <div className="grid gap-3 sm:grid-cols-3">
              <TextField
                label="From (HH:MM)"
                value={draft.quietStart}
                onChange={(e) => set({ quietStart: e.target.value })}
              />
              <TextField
                label="Until (HH:MM)"
                value={draft.quietEnd}
                onChange={(e) => set({ quietEnd: e.target.value })}
              />
              <TextField
                label="Time zone"
                value={draft.quietTz}
                onChange={(e) => set({ quietTz: e.target.value })}
                hint="IANA name, e.g. Europe/Berlin."
              />
            </div>
          )}
        </div>

        <TextField
          label="Title template"
          value={draft.titleTemplate}
          placeholder="{{rule}}"
          onChange={(e) => set({ titleTemplate: e.target.value })}
        />
        <TextArea
          label="Message template"
          rows={3}
          maxLength={2000}
          value={draft.bodyTemplate}
          onChange={(bodyTemplate) => set({ bodyTemplate })}
          hint="Placeholders: {{value}}, {{data.text}}, {{data.address}}, {{device}}, {{freq_mhz}}, {{time}}, {{rule}}, {{where}}."
        />

        <fieldset className="flex flex-col gap-2">
          <legend className="text-xs font-medium text-fg-muted">Send to</legend>
          {targets.length === 0 && <p className="text-xs text-fg-subtle">Create a target first.</p>}
          {targets.map((t) => (
            <Toggle
              key={t.id}
              label={`${t.name} (${t.kind})`}
              checked={draft.targetIds.includes(t.id)}
              onChange={(on) =>
                set({
                  targetIds: on
                    ? [...draft.targetIds, t.id]
                    : draft.targetIds.filter((id) => id !== t.id),
                })
              }
            />
          ))}
        </fieldset>

        {error && <Alert>{error}</Alert>}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void save()} disabled={busy}>
            {busy ? "Saving…" : "Save rule"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
