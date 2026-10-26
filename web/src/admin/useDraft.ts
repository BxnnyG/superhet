import { useState } from "react";

export interface Draft<T> {
  /** The values shown in the form: the unsaved edit, or the saved values. */
  value: T | null;
  set: (next: T) => void;
  dirty: boolean;
  discard: () => void;
}

/**
 * Edit state for a settings form over a loaded document. Edits are derived from the saved value:
 * when a new saved value arrives (after saving, or a reload) the old edit is dropped, no effect
 * needed.
 */
export function useDraft<T>(saved: T | null): Draft<T> {
  const [edit, setEdit] = useState<{ base: T | null; value: T | null }>({
    base: null,
    value: null,
  });
  const value = edit.value !== null && edit.base === saved ? edit.value : saved;
  const dirty = value !== null && saved !== null && JSON.stringify(value) !== JSON.stringify(saved);
  return {
    value,
    dirty,
    set: (next) => setEdit({ base: saved, value: next }),
    discard: () => setEdit({ base: null, value: null }),
  };
}
