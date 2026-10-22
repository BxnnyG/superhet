import { useState, useSyncExternalStore, type FormEvent } from "react";
import { formatFrequency, parseFrequency } from "../spectrum/freq";
import { Alert, Button, Dialog, Select, TextField, toasts } from "../ui";
import { VFO_MODES, type VfoMode } from "../vfo/api";
import { MODE_LABEL } from "../vfo/model";
import {
  BOOKMARK_COLORS,
  NAME_MAX,
  parseTags,
  toInput,
  type Bookmark,
  type BookmarkInput,
} from "./model";
import type { BookmarkStore } from "./store";

const NO_MODE = "";
const NO_COLOR = "";

/** MHz text for the field: "433.5", "100.1". */
function mhzText(hz: number): string {
  return String(parseFloat((hz / 1e6).toFixed(6)));
}

interface FormProps {
  title: string;
  submitLabel: string;
  initial: BookmarkInput;
  onCancel: () => void;
  /** Resolves with an error text to show, or null when saved. */
  onSubmit: (input: BookmarkInput) => Promise<string | null>;
}

function BookmarkForm({ title, submitLabel, initial, onCancel, onSubmit }: FormProps) {
  const [name, setName] = useState(initial.name);
  const [freq, setFreq] = useState(mhzText(initial.freqHz));
  const [mode, setMode] = useState<string>(initial.mode ?? NO_MODE);
  const [bandwidth, setBandwidth] = useState(
    initial.bandwidthHz === null ? "" : String(parseFloat((initial.bandwidthHz / 1e3).toFixed(3))),
  );
  const [tags, setTags] = useState(initial.tags.join(", "));
  const [color, setColor] = useState(initial.color ?? NO_COLOR);
  const [notes, setNotes] = useState(initial.notes ?? "");
  const [errors, setErrors] = useState<{ name?: string; freq?: string; bandwidth?: string }>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const next: typeof errors = {};
    const trimmed = name.trim();
    if (trimmed === "") next.name = "Enter a name.";
    const freqHz = parseFrequency(freq);
    if (freqHz === null) next.freq = "Enter a frequency, e.g. 433.5 or 145.5M.";
    let bandwidthHz: number | null = null;
    if (bandwidth.trim() !== "") {
      const kHz = Number(bandwidth.trim().replace(",", "."));
      if (!Number.isFinite(kHz) || kHz <= 0 || kHz > 5_000)
        next.bandwidth = "Enter kHz, e.g. 12.5.";
      else bandwidthHz = Math.round(kHz * 1000);
    }
    setErrors(next);
    if (next.name || next.freq || next.bandwidth || freqHz === null) return;
    setPending(true);
    setFailure(null);
    const result = await onSubmit({
      name: trimmed,
      freqHz,
      mode: VFO_MODES.find((m) => m === mode) ?? null,
      bandwidthHz,
      tags: parseTags(tags),
      color: color === NO_COLOR ? null : color,
      notes: notes.trim() === "" ? null : notes.trim(),
    });
    setPending(false);
    if (result !== null) setFailure(result);
  };

  return (
    <Dialog title={title} onClose={onCancel}>
      <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3" noValidate>
        <TextField
          label="Name"
          value={name}
          maxLength={NAME_MAX}
          error={errors.name}
          autoComplete="off"
          onChange={(e) => setName(e.target.value)}
        />
        <TextField
          label="Frequency (MHz)"
          value={freq}
          error={errors.freq}
          inputMode="decimal"
          autoComplete="off"
          hint={freqHint(freq)}
          onChange={(e) => setFreq(e.target.value)}
        />
        <div className="grid grid-cols-2 gap-3">
          <Select
            label="Mode"
            value={mode}
            onChange={setMode}
            options={[
              { value: NO_MODE, label: "Keep current" },
              ...VFO_MODES.map((m: VfoMode) => ({ value: m, label: MODE_LABEL[m] })),
            ]}
          />
          <TextField
            label="Bandwidth (kHz)"
            value={bandwidth}
            error={errors.bandwidth}
            inputMode="decimal"
            autoComplete="off"
            placeholder="mode default"
            onChange={(e) => setBandwidth(e.target.value)}
          />
        </div>
        <TextField
          label="Tags"
          value={tags}
          autoComplete="off"
          hint="Comma separated"
          onChange={(e) => setTags(e.target.value)}
        />
        <Select
          label="Marker colour"
          value={color}
          onChange={setColor}
          options={[
            { value: NO_COLOR, label: "Default" },
            ...BOOKMARK_COLORS.map((c) => ({ value: c.value, label: c.label })),
          ]}
        />
        <TextField
          label="Notes"
          value={notes}
          autoComplete="off"
          onChange={(e) => setNotes(e.target.value)}
        />
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

function freqHint(text: string): string | undefined {
  const hz = parseFrequency(text);
  return hz === null ? undefined : `${formatFrequency(hz)} MHz`;
}

/** Renders the add/edit dialog while the store has an editor target. Mount once in the app. */
export function BookmarkEditorHost({ store }: { store: BookmarkStore }) {
  const { editor } = useSyncExternalStore(store.subscribe, store.getSnapshot);
  if (editor === null) return null;
  const close = () => store.closeEditor();
  if (editor.kind === "create") {
    return (
      <BookmarkForm
        key="create"
        title="Add bookmark"
        submitLabel="Save"
        initial={editor.draft}
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
