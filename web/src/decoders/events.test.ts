import { describe, expect, it } from "vitest";
import {
  INITIAL_LOG,
  MAX_EVENTS,
  eventNote,
  eventTarget,
  formatDuration,
  formatMhz,
  logReducer,
  matches,
  mergeEvents,
  parseEvent,
  type DecoderEvent,
  type LogState,
} from "./events";

const ev = (id: number, over: Partial<DecoderEvent> = {}): DecoderEvent => ({
  id,
  tsMs: 1_700_000_000_000 + id * 1000,
  device: "rtl0",
  vfo: 1,
  freqHz: 433_500_000,
  mode: "nfm",
  kind: "dtmf",
  value: `v${id}`,
  meta: {},
  ...over,
});

const ids = (s: Pick<LogState, "events">) => s.events.map((e) => e.id);

describe("parseEvent", () => {
  const raw = {
    id: 5,
    ts_ms: 1_700_000_000_123,
    device: "rtl0",
    vfo: 2,
    freq_hz: 99_200_000,
    mode: "wfm",
    kind: "rds_ps",
    value: "RADIO X",
    meta: { pi: "D220" },
  };

  it("accepts a well-formed event", () => {
    expect(parseEvent(raw)).toEqual({
      id: 5,
      tsMs: 1_700_000_000_123,
      device: "rtl0",
      vfo: 2,
      freqHz: 99_200_000,
      mode: "wfm",
      kind: "rds_ps",
      value: "RADIO X",
      meta: { pi: "D220" },
    });
  });

  it("tolerates a missing frequency, an unknown mode and bad meta", () => {
    const e = parseEvent({ ...raw, freq_hz: null, mode: "x", meta: [] });
    expect(e?.freqHz).toBeNull();
    expect(e?.mode).toBeNull();
    expect(e?.meta).toEqual({});
  });

  it.each([
    null,
    "x",
    { ...raw, id: "5" },
    { ...raw, kind: "other" },
    { ...raw, value: 3 },
    { ...raw, vfo: -1 },
    { ...raw, ts_ms: NaN },
  ])("rejects %j", (bad) => {
    expect(parseEvent(bad)).toBeNull();
  });
});

describe("mergeEvents", () => {
  it("de-duplicates by id and sorts newest first", () => {
    const merged = mergeEvents([ev(1), ev(3)], [ev(2), ev(3)]);
    expect(merged.map((e) => e.id)).toEqual([3, 2, 1]);
  });

  it("orders equal times by id and caps the list", () => {
    const same = (id: number) => ev(id, { tsMs: 1000 });
    expect(mergeEvents([same(1), same(2)], []).map((e) => e.id)).toEqual([2, 1]);
    const many = Array.from({ length: MAX_EVENTS + 5 }, (_, i) => ev(i + 1));
    const capped = mergeEvents(many, []);
    expect(capped).toHaveLength(MAX_EVENTS);
    expect(capped[0]?.id).toBe(MAX_EVENTS + 5);
  });
});

describe("matches", () => {
  it("filters by kind and by case-insensitive value", () => {
    const e = ev(1, { kind: "rds_rt", value: "Now Playing: Song" });
    expect(matches({ kinds: [], q: "" }, e)).toBe(true);
    expect(matches({ kinds: ["rds_rt"], q: "" }, e)).toBe(true);
    expect(matches({ kinds: ["dtmf"], q: "" }, e)).toBe(false);
    expect(matches({ kinds: [], q: " playing " }, e)).toBe(true);
    expect(matches({ kinds: [], q: "absent" }, e)).toBe(false);
  });
});

describe("logReducer", () => {
  const loaded = (events: DecoderEvent[], next: string | null = null): LogState => {
    const s = logReducer(INITIAL_LOG, { type: "query" });
    return logReducer(s, { type: "page", gen: s.gen, events, next });
  };

  it("loads a first page and marks the log loaded", () => {
    const s = loaded([ev(2), ev(1)], "c1");
    expect(ids(s)).toEqual([2, 1]);
    expect(s).toMatchObject({ loaded: true, loading: false, next: "c1", error: null });
  });

  it("appends live events in order without duplicates", () => {
    let s = loaded([ev(2), ev(1)]);
    s = logReducer(s, { type: "live", event: ev(3) });
    s = logReducer(s, { type: "live", event: ev(3) });
    // An event stamped earlier than the newest (a burst's first digit) slots in by time.
    s = logReducer(s, { type: "live", event: ev(9, { tsMs: 1_700_000_002_500 }) });
    expect(ids(s)).toEqual([3, 9, 2, 1]);
  });

  it("keeps live events that arrived while the page was loading", () => {
    let s = logReducer(INITIAL_LOG, { type: "query" });
    const gen = s.gen;
    s = logReducer(s, { type: "live", event: ev(5) });
    s = logReducer(s, { type: "page", gen, events: [ev(5), ev(4)], next: null });
    expect(ids(s)).toEqual([5, 4]);
  });

  it("drops replies of an older query", () => {
    let s = logReducer(INITIAL_LOG, { type: "query" });
    const stale = s.gen;
    s = logReducer(s, { type: "query", filter: { kinds: ["dtmf"], q: "" } });
    s = logReducer(s, { type: "page", gen: stale, events: [ev(1)], next: null });
    expect(s.events).toEqual([]);
    expect(s.loading).toBe(true);
    s = logReducer(s, { type: "failed", gen: stale, error: "late" });
    expect(s.error).toBeNull();
  });

  it("a new filter resets the list and applies to live events", () => {
    let s = loaded([ev(2), ev(1)]);
    s = logReducer(s, { type: "query", filter: { kinds: ["rds_ps"], q: "" } });
    expect(s).toMatchObject({ events: [], loaded: false, loading: true, next: null });
    s = logReducer(s, { type: "live", event: ev(3) });
    expect(s.events).toEqual([]);
    s = logReducer(s, { type: "live", event: ev(4, { kind: "rds_ps" }) });
    expect(ids(s)).toEqual([4]);
  });

  it("a plain reload keeps the shown events", () => {
    let s = loaded([ev(2)]);
    s = logReducer(s, { type: "query" });
    expect(ids(s)).toEqual([2]);
    expect(s.loading).toBe(true);
  });

  it("pause holds live events back until resume", () => {
    let s = loaded([ev(1)]);
    s = logReducer(s, { type: "pause" });
    s = logReducer(s, { type: "live", event: ev(2) });
    s = logReducer(s, { type: "live", event: ev(3) });
    expect(ids(s)).toEqual([1]);
    expect(s.pending).toHaveLength(2);
    s = logReducer(s, { type: "resume" });
    expect(ids(s)).toEqual([3, 2, 1]);
    expect(s).toMatchObject({ paused: false, pending: [] });
  });

  it("older pages are appended after the current list", () => {
    let s = loaded([ev(5), ev(4)], "c1");
    s = { ...s, loading: true };
    s = logReducer(s, { type: "older", gen: s.gen, events: [ev(3), ev(2)], next: null });
    expect(ids(s)).toEqual([5, 4, 3, 2]);
    expect(s).toMatchObject({ next: null, loading: false });
  });

  it("clearing empties everything and ignores pending replies", () => {
    let s = loaded([ev(1)], "c");
    s = logReducer(s, { type: "pause" });
    s = logReducer(s, { type: "live", event: ev(2) });
    const before = s.gen;
    s = logReducer(s, { type: "cleared" });
    expect(s).toMatchObject({ events: [], pending: [], next: null, loaded: true });
    expect(s.gen).toBe(before + 1);
    s = logReducer(s, { type: "page", gen: before, events: [ev(1)], next: null });
    expect(s.events).toEqual([]);
  });

  it("an error is recorded for the current query only", () => {
    let s = logReducer(INITIAL_LOG, { type: "query" });
    s = logReducer(s, { type: "failed", gen: s.gen, error: "HTTP 503" });
    expect(s).toMatchObject({ error: "HTTP 503", loading: false, loaded: true });
    s = logReducer(s, { type: "query" });
    expect(s.error).toBeNull();
  });
});

describe("display helpers", () => {
  it("formats MHz with at least three decimals", () => {
    expect(formatMhz(433_500_000)).toBe("433.500");
    expect(formatMhz(99_200_000)).toBe("99.200");
    expect(formatMhz(433_512_500)).toBe("433.5125");
    expect(formatMhz(446_006_250)).toBe("446.00625");
  });

  it("formats durations", () => {
    expect(formatDuration(1234)).toBe("1.2 s");
    expect(formatDuration(42_000)).toBe("42 s");
    expect(formatDuration(245_000)).toBe("4 min 05 s");
  });

  it("describes tone edges", () => {
    expect(eventNote(ev(1, { kind: "ctcss", meta: { edge: "start" } }))).toBe("on");
    expect(eventNote(ev(1, { kind: "ctcss", meta: { edge: "stop", duration_ms: 8000 } }))).toBe(
      "off after 8.0 s",
    );
    expect(eventNote(ev(1))).toBeNull();
  });

  it("derives a tuning target", () => {
    expect(eventTarget(ev(1))).toEqual({ freqHz: 433_500_000, mode: "nfm" });
    expect(eventTarget(ev(1, { mode: null }))).toEqual({ freqHz: 433_500_000 });
    expect(eventTarget(ev(1, { freqHz: null }))).toBeNull();
  });
});
