import { useState, useSyncExternalStore, type FormEvent, type ReactNode } from "react";
import { CTCSS_TONES, formatCtcss } from "../audio/tones";
import { formatFrequency, parseFrequency } from "../spectrum/freq";
import { Alert, Button, Dialog, Select, TextField, Toggle, toasts } from "../ui";
import { VFO_MODES, type VfoMode } from "../vfo/api";
import { MODE_LABEL, STEPS_HZ, formatStep } from "../vfo/model";
import { fromForm, toForm, type FormErrors, type FormState, type SquelchKind } from "./editorForm";
import {
  BOOKMARK_COLORS,
  DIGITAL_PROTOCOLS,
  MAX_TIMESLOT,
  NAME_MAX,
  PROTOCOL_LABEL,
  toInput,
  type Bookmark,
  type BookmarkGroup,
  type BookmarkInput,
  type DcsPolarity,
  type ToneMode,
} from "./model";
import type { BookmarkStore } from "./store";

interface FormProps {
  title: string;
  submitLabel: string;
  initial: BookmarkInput;
  groups: readonly BookmarkGroup[];
  onCancel: () => void;
  /** Resolves with an error text to show, or null when saved. */
  onSubmit: (input: BookmarkInput) => Promise<string | null>;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="flex min-w-0 flex-col gap-3 border-t border-border pt-3">
      <legend className="float-left mb-0 w-full text-[11px] font-medium tracking-wide text-fg-subtle uppercase">
        {title}
      </legend>
      <div className="clear-both flex flex-col gap-3">{children}</div>
    </fieldset>
  );
}

const SQUELCH_OPTIONS: readonly { value: SquelchKind; label: string }[] = [
  { value: "keep", label: "Keep current" },
  { value: "fixed", label: "Fixed level" },
  { value: "auto", label: "Auto (noise floor)" },
];

const TONE_OPTIONS: readonly { value: "" | ToneMode; label: string }[] = [
  { value: "", label: "Not set" },
  { value: "none", label: "No sub-tone" },
  { value: "ctcss", label: "CTCSS" },
  { value: "dcs", label: "DCS (store only)" },
];

function BookmarkForm({ title, submitLabel, initial, groups, onCancel, onSubmit }: FormProps) {
  const [f, setF] = useState<FormState>(() => toForm(initial));
  const [errors, setErrors] = useState<FormErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const set = (patch: Partial<FormState>) => setF((prev) => ({ ...prev, ...patch }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const result = fromForm(f);
    if ("errors" in result) {
      setErrors(result.errors);
      return;
    }
    setErrors({});
    setPending(true);
    setFailure(null);
    const error = await onSubmit(result.input);
    setPending(false);
    if (error !== null) setFailure(error);
  };

  // A stored tone that is not one of the standard 50 stays selectable.
  const ctcssOptions = [
    ...(f.ctcss !== "" && !CTCSS_TONES.includes(Number(f.ctcss)) ? [Number(f.ctcss)] : []),
    ...CTCSS_TONES,
  ].map((hz) => ({ value: String(hz), label: formatCtcss(hz) }));
  const slotMax = f.protocol === "" ? 0 : MAX_TIMESLOT[f.protocol];

  return (
    <Dialog title={title} onClose={onCancel}>
      <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3" noValidate>
        <TextField
          label="Name"
          value={f.name}
          maxLength={NAME_MAX}
          error={errors.name}
          autoComplete="off"
          onChange={(e) => set({ name: e.target.value })}
        />
        <div className="grid grid-cols-2 gap-3">
          <TextField
            label="Frequency (MHz)"
            value={f.freq}
            error={errors.freq}
            inputMode="decimal"
            autoComplete="off"
            hint={freqHint(f.freq)}
            onChange={(e) => set({ freq: e.target.value })}
          />
          <Select
            label="Group"
            value={f.groupId}
            onChange={(groupId) => set({ groupId })}
            options={[
              { value: "", label: "No group" },
              ...groups.map((g) => ({ value: g.id, label: g.name })),
            ]}
          />
        </div>

        <Section title="Receiver">
          <div className="grid grid-cols-2 gap-3">
            <Select
              label="Mode"
              value={f.mode}
              onChange={(mode) => set({ mode })}
              options={[
                { value: "", label: "Keep current" },
                ...VFO_MODES.map((m: VfoMode) => ({ value: m, label: MODE_LABEL[m] })),
              ]}
            />
            <TextField
              label="Bandwidth (kHz)"
              value={f.bandwidth}
              error={errors.bandwidth}
              inputMode="decimal"
              autoComplete="off"
              placeholder="mode default"
              onChange={(e) => set({ bandwidth: e.target.value })}
            />
            <Select
              label="Tuning step"
              value={f.step}
              onChange={(step) => set({ step })}
              options={[{ value: "", label: "Mode default" }, ...stepOptions(f.step)]}
            />
            <Select
              label="Squelch"
              value={f.squelchKind}
              onChange={(squelchKind) => set({ squelchKind: squelchKind as SquelchKind })}
              options={SQUELCH_OPTIONS.map((o) => ({ ...o }))}
            />
          </div>
          {f.squelchKind !== "keep" && (
            <TextField
              label="Squelch level (dBFS)"
              value={f.squelch}
              error={errors.squelch}
              inputMode="decimal"
              autoComplete="off"
              placeholder="-80"
              hint={
                f.squelchKind === "auto"
                  ? "Auto re-measures the noise floor; this is the last measured level."
                  : undefined
              }
              onChange={(e) => set({ squelch: e.target.value })}
            />
          )}
        </Section>

        <Section title="Sub-tone">
          <Select
            label="Sub-tone"
            value={f.toneMode}
            onChange={(toneMode) => set({ toneMode: toneMode as "" | ToneMode })}
            options={TONE_OPTIONS.map((o) => ({ ...o }))}
          />
          {f.toneMode === "ctcss" && (
            <Select
              label="CTCSS tone"
              value={f.ctcss}
              onChange={(ctcss) => set({ ctcss })}
              options={[{ value: "", label: "Choose…" }, ...ctcssOptions]}
            />
          )}
          {errors.ctcss && <Alert tone="danger">{errors.ctcss}</Alert>}
          {f.toneMode === "dcs" && (
            <>
              <div className="grid grid-cols-2 gap-3">
                <TextField
                  label="DCS code"
                  value={f.dcsCode}
                  error={errors.dcs}
                  maxLength={3}
                  inputMode="numeric"
                  autoComplete="off"
                  placeholder="023"
                  onChange={(e) => set({ dcsCode: e.target.value })}
                />
                <Select
                  label="DCS polarity"
                  value={f.dcsPolarity}
                  onChange={(p) => set({ dcsPolarity: p as DcsPolarity })}
                  options={[
                    { value: "N", label: "Normal (N)" },
                    { value: "R", label: "Inverted (R)" },
                  ]}
                />
              </div>
              <p className="text-xs text-fg-subtle">
                Store only: the receiver does not detect DCS yet, so it is kept for export and for
                later use.
              </p>
            </>
          )}
        </Section>

        <Section title="Digital channel">
          <Select
            label="Protocol"
            value={f.protocol}
            onChange={(protocol) => set({ protocol: protocol as FormState["protocol"] })}
            options={[
              { value: "", label: "Analogue" },
              ...DIGITAL_PROTOCOLS.map((p) => ({ value: p, label: PROTOCOL_LABEL[p] })),
            ]}
          />
          {f.protocol !== "" && (
            <>
              <div className="grid grid-cols-2 gap-3">
                {slotMax > 0 && (
                  <Select
                    label="Timeslot"
                    value={f.timeslot}
                    onChange={(timeslot) => set({ timeslot })}
                    options={[
                      { value: "", label: "Any" },
                      ...Array.from({ length: slotMax }, (_, n) => ({
                        value: String(n + 1),
                        label: `TS ${n + 1}`,
                      })),
                    ]}
                  />
                )}
                <TextField
                  label={f.protocol === "tetra" ? "Talkgroup / GSSI" : "Talkgroup"}
                  value={f.talkgroup}
                  error={errors.talkgroup}
                  inputMode="numeric"
                  autoComplete="off"
                  onChange={(e) => set({ talkgroup: e.target.value })}
                />
                {f.protocol === "dmr" && (
                  <TextField
                    label="Colour code"
                    value={f.colourCode}
                    error={errors.colourCode}
                    inputMode="numeric"
                    autoComplete="off"
                    onChange={(e) => set({ colourCode: e.target.value })}
                  />
                )}
                {f.protocol === "tetra" && (
                  <>
                    <TextField
                      label="MCC"
                      value={f.mcc}
                      error={errors.mcc}
                      inputMode="numeric"
                      autoComplete="off"
                      onChange={(e) => set({ mcc: e.target.value })}
                    />
                    <TextField
                      label="MNC"
                      value={f.mnc}
                      error={errors.mnc}
                      inputMode="numeric"
                      autoComplete="off"
                      onChange={(e) => set({ mnc: e.target.value })}
                    />
                  </>
                )}
              </div>
              <TextField
                label="Decoder plugin"
                value={f.decoder}
                error={errors.decoder}
                autoComplete="off"
                placeholder="plugin id"
                onChange={(e) => set({ decoder: e.target.value })}
              />
            </>
          )}
        </Section>

        <Section title="Organise">
          <TextField
            label="Tags"
            value={f.tags}
            autoComplete="off"
            hint="Comma separated"
            onChange={(e) => set({ tags: e.target.value })}
          />
          <div className="grid grid-cols-2 items-end gap-3">
            <Select
              label="Marker colour"
              value={f.color}
              onChange={(color) => set({ color })}
              options={[
                { value: "", label: "Default" },
                ...BOOKMARK_COLORS.map((c) => ({ value: c.value, label: c.label })),
              ]}
            />
            <Toggle
              label="Favourite"
              checked={f.favourite}
              onChange={(favourite) => set({ favourite })}
            />
          </div>
          <TextField
            label="Notes"
            value={f.notes}
            autoComplete="off"
            onChange={(e) => set({ notes: e.target.value })}
          />
        </Section>

        <Section title="Scanning">
          <Toggle
            label="Include in scans"
            checked={f.scanInclude}
            onChange={(scanInclude) => set({ scanInclude })}
          />
          {f.scanInclude && (
            <div className="grid grid-cols-2 gap-3">
              <Select
                label="Priority"
                value={f.scanPriority}
                onChange={(scanPriority) => set({ scanPriority })}
                options={[
                  { value: "", label: "Normal" },
                  ...Array.from({ length: 10 }, (_, n) => ({
                    value: String(n),
                    label: String(n),
                  })),
                ]}
              />
              <TextField
                label="Dwell time (ms)"
                value={f.scanDwell}
                error={errors.scanDwell}
                inputMode="numeric"
                autoComplete="off"
                placeholder="default"
                onChange={(e) => set({ scanDwell: e.target.value })}
              />
            </div>
          )}
        </Section>

        {failure !== null && <Alert tone="danger">{failure}</Alert>}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={pending}>
            {submitLabel}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

/** The selectable steps, plus a stored custom one so it is not lost on save. */
function stepOptions(current: string): { value: string; label: string }[] {
  const steps: number[] = [...STEPS_HZ];
  const n = Number(current);
  if (current !== "" && Number.isFinite(n) && !steps.includes(n)) steps.push(n);
  return steps.sort((a, b) => a - b).map((hz) => ({ value: String(hz), label: formatStep(hz) }));
}

function freqHint(text: string): string | undefined {
  const hz = parseFrequency(text);
  return hz === null ? undefined : `${formatFrequency(hz)} MHz`;
}

/** Renders the add/edit dialog while the store has an editor target. Mount once in the app. */
export function BookmarkEditorHost({ store }: { store: BookmarkStore }) {
  const { editor, groups } = useSyncExternalStore(store.subscribe, store.getSnapshot);
  if (editor === null) return null;
  const close = () => store.closeEditor();
  if (editor.kind === "create") {
    return (
      <BookmarkForm
        key="create"
        title="Add bookmark"
        submitLabel="Save"
        initial={editor.draft}
        groups={groups}
        onCancel={close}
        onSubmit={async (input) => {
          const error = await store.create(input);
          if (error !== null) return error;
          toasts.push({ tone: "success", title: `Bookmarked “${input.name}”`, key: "bookmark" });
          close();
          return null;
        }}
      />
    );
  }
  const b: Bookmark = editor.bookmark;
  return (
    <BookmarkForm
      key={b.id}
      title="Edit bookmark"
      submitLabel="Save"
      initial={toInput(b)}
      groups={groups}
      onCancel={close}
      onSubmit={async (input) => {
        const error = await store.update(b.id, input);
        if (error !== null) return error;
        close();
        return null;
      }}
    />
  );
}
