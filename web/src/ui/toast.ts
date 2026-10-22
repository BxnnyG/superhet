// Transient, non-blocking notices. A small external store so controllers (plain TS) can raise a
// notice without React; <Toaster> renders it. Notices with the same `key` replace each other, so
// a burst of identical rejections (a drag against the tuning lock) shows one toast.

export type ToastTone = "info" | "success" | "warn" | "danger";

export interface ToastInput {
  tone?: ToastTone;
  title: string;
  body?: string;
  /** Replaces a visible toast with the same key instead of stacking. */
  key?: string;
  /** Auto-dismiss delay; 0 keeps it until dismissed. Default 6 s (danger 10 s). */
  durationMs?: number;
}

export interface ToastItem {
  id: number;
  key: string | null;
  tone: ToastTone;
  title: string;
  body: string | null;
}

const MAX_VISIBLE = 4;

export class ToastStore {
  private items: readonly ToastItem[] = [];
  private nextId = 1;
  private readonly timers = new Map<number, ReturnType<typeof setTimeout>>();
  private readonly listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): readonly ToastItem[] => this.items;

  push(input: ToastInput): number {
    const tone = input.tone ?? "info";
    const key = input.key ?? null;
    let id = this.nextId++;
    const old = key === null ? undefined : this.items.find((t) => t.key === key);
    if (old) {
      id = old.id;
      clearTimeout(this.timers.get(id));
    }
    const item: ToastItem = { id, key, tone, title: input.title, body: input.body ?? null };
    let next = old ? this.items.map((t) => (t.id === id ? item : t)) : [...this.items, item];
    while (next.length > MAX_VISIBLE) {
      const dropped = next[0];
      if (dropped) {
        clearTimeout(this.timers.get(dropped.id));
        this.timers.delete(dropped.id);
      }
      next = next.slice(1);
    }
    this.items = next;
    const ms = input.durationMs ?? (tone === "danger" ? 10_000 : 6_000);
    if (ms > 0)
      this.timers.set(
        id,
        setTimeout(() => this.dismiss(id), ms),
      );
    this.emit();
    return id;
  }

  dismiss(id: number): void {
    clearTimeout(this.timers.get(id));
    this.timers.delete(id);
    if (!this.items.some((t) => t.id === id)) return;
    this.items = this.items.filter((t) => t.id !== id);
    this.emit();
  }

  clear(): void {
    for (const t of this.timers.values()) clearTimeout(t);
    this.timers.clear();
    this.items = [];
    this.emit();
  }

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }
}

/** App-wide store used by the controllers and rendered by <Toaster>. */
export const toasts = new ToastStore();
