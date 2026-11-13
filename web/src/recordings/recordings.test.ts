import { describe, expect, it, vi } from "vitest";
import { RecordingError, createRecordingsApi, fileUrl, parseConfig, parseRecording } from "./api";
import { endReason, formatBytes, formatClock, replayChoices, runningFor, subtitle } from "./model";

const wire = {
  id: 7,
  kind: "audio",
  source: "scanner",
  state: "recording",
  title: "Tower",
  device: "dev0",
  vfo: 2,
  freq_hz: 118_300_000,
  mode: "am",
  started_ms: 1000,
  stopped_ms: null,
  duration_ms: 65_000,
  size_bytes: 2048,
  keep: true,
  meta: { end_reason: "limit" },
};

describe("recordings model", () => {
  it("parses a recording and rejects garbage", () => {
    const r = parseRecording(wire);
    expect(r).toMatchObject({ id: 7, vfo: 2, freqHz: 118_300_000, keep: true, owner: null });
    expect(parseRecording({ ...wire, kind: "video" })).toBeNull();
    expect(parseRecording({ ...wire, state: "weird" })).toBeNull();
    expect(parseRecording(null)).toBeNull();
  });

  it("formats durations, sizes and subtitles", () => {
    expect(formatClock(65_000)).toBe("1:05");
    expect(formatClock(3_723_000)).toBe("1:02:03");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(3.4 * 1024 * 1024)).toBe("3.40 MB");
    const r = parseRecording(wire);
    expect(r).not.toBeNull();
    if (r === null) return;
    expect(subtitle(r)).toContain("AM");
    expect(subtitle(r)).toContain("Scanner hit");
    expect(endReason(r)).toMatch(/limit/);
    expect(runningFor([r], 2)).toBe(r);
    expect(runningFor([r], 3)).toBeNull();
  });

  it("offers only replay lengths the server buffers", () => {
    expect(replayChoices(60)).toEqual([10, 30, 60]);
    expect(replayChoices(30)).toEqual([10, 30]);
    expect(replayChoices(5)).toEqual([]);
  });

  it("parses the config with defaults", () => {
    expect(
      parseConfig({ available: true, replay_seconds: 120, hit_recording: ["dev0", 5] }),
    ).toMatchObject({
      available: true,
      replaySeconds: 120,
      hitRecording: ["dev0"],
    });
    expect(parseConfig(undefined).replaySeconds).toBe(60);
  });

  it("builds file URLs", () => {
    expect(fileUrl(3)).toBe("/api/v1/recordings/3/file");
    expect(fileUrl(3, { download: true })).toBe("/api/v1/recordings/3/file?download=true");
    expect(fileUrl(3, { part: "meta" })).toBe("/api/v1/recordings/3/file?part=meta");
  });
});

describe("recordings api", () => {
  const reply = (status: number, body: unknown) =>
    vi.fn(async () => new Response(JSON.stringify(body), { status }));

  it("lists with query parameters", async () => {
    const f = reply(200, { recordings: [wire], next_before: 7 });
    const api = createRecordingsApi(f as unknown as typeof fetch);
    const page = await api.list({ dev: "dev0", kind: "audio", q: "tow" });
    expect(page.items).toHaveLength(1);
    expect(page.nextBefore).toBe(7);
    const url = String((f.mock.calls[0] as unknown[])[0]);
    expect(url).toContain("dev=dev0");
    expect(url).toContain("kind=audio");
  });

  it("starts an audio recording with the wire body", async () => {
    const f = reply(201, wire);
    const api = createRecordingsApi(f as unknown as typeof fetch);
    await api.start("dev0", { kind: "audio", vfo: 2, gated: true });
    const init = (f.mock.calls[0] as unknown[])[1] as RequestInit;
    expect(JSON.parse(String(init.body))).toMatchObject({ kind: "audio", vfo: 2, gated: true });
  });

  it("turns refusals into readable errors", async () => {
    const api = createRecordingsApi(
      reply(507, { ok: false, code: "quota_full", error: "x" }) as unknown as typeof fetch,
    );
    await expect(api.start("dev0", { kind: "audio", vfo: 1, gated: false })).rejects.toMatchObject({
      status: 507,
      code: "quota_full",
    });
    const forbidden = createRecordingsApi(
      reply(403, { error: "forbidden" }) as unknown as typeof fetch,
    );
    await expect(forbidden.remove(1)).rejects.toBeInstanceOf(RecordingError);
  });
});
