import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchInfo, parseInfo, parseState } from "./api";
import { embedCode, embedPath, receiverPath, shareApi, sharePageOf } from "./paths";

vi.mock("./ShareSpectrum", () => ({ ShareSpectrum: () => <div data-testid="spectrum" /> }));

afterEach(() => vi.restoreAllMocks());

describe("share paths", () => {
  it("recognises the two public pages and nothing else", () => {
    expect(sharePageOf("/r/abc_-123")).toEqual({ kind: "receiver", token: "abc_-123" });
    expect(sharePageOf("/embed/abc/")).toEqual({ kind: "embed", token: "abc" });
    for (const other of ["/", "/admin", "/r/", "/r/a/b", "/invite/x", "/embedx/y", "/r/%E0%A4%A"]) {
      expect(sharePageOf(other)).toBeNull();
    }
  });

  it("keeps the token in the path and out of queries", () => {
    expect(receiverPath("a b")).toBe("/r/a%20b");
    expect(embedPath("t")).toBe("/embed/t");
    const api = shareApi("t");
    expect(api.info).toBe("/api/v1/share/t");
    expect(api.audio({ protocol: "https:", host: "sdr.example" })).toBe(
      "wss://sdr.example/api/v1/share/t/ws/audio",
    );
    expect(api.spectrum({ protocol: "http:", host: "h:1" })).toBe(
      "ws://h:1/api/v1/share/t/ws/spectrum",
    );
    expect(embedCode("t", "https://sdr.example")).toContain('src="https://sdr.example/embed/t"');
  });
});

describe("share api parsing", () => {
  it("parses a following and a fixed link", () => {
    const follow = parseInfo({
      device: "dev0",
      title: "T",
      features: ["audio", "bogus"],
      tuning: { mode: "follow" },
      listeners: 3,
      max_listeners: 5,
    });
    expect(follow).toMatchObject({ features: ["audio"], tuning: { mode: "follow" }, listeners: 3 });
    const fixed = parseInfo({
      device: "dev0",
      features: [],
      tuning: { mode: "fixed", freq_hz: 1e8, demod: "wfm", range: { min_hz: 9e7, max_hz: 11e7 } },
    });
    expect(fixed?.tuning).toEqual({
      mode: "fixed",
      freqHz: 1e8,
      demod: "wfm",
      range: { minHz: 9e7, maxHz: 11e7 },
    });
    expect(parseInfo(null)).toBeNull();
  });

  it("keeps only what the page needs from the state", () => {
    const state = parseState({
      center_hz: 1e8,
      vfos: [{ id: 1, freq_hz: 1.001e8, mode: "wfm" }, { id: "x" }],
      plugin_streams: [{ id: "i1", plugin_id: "dab", audio_vfo: 1073741825 }],
    });
    expect(state?.vfos).toEqual([{ id: 1, freqHz: 1.001e8, mode: "wfm" }]);
    expect(state?.pluginStreams).toEqual([{ id: "i1", pluginId: "dab", audioVfo: 1073741825 }]);
  });

  it("maps answers to problems without credentials", async () => {
    const answer = (status: number, body: unknown) =>
      vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status }));
    expect(await fetchInfo("t", undefined, answer(410, { error: "revoked" }))).toEqual({
      ok: false,
      problem: "revoked",
    });
    expect(await fetchInfo("t", undefined, answer(410, { error: "expired" }))).toEqual({
      ok: false,
      problem: "expired",
    });
    expect(await fetchInfo("t", undefined, answer(404, {}))).toEqual({
      ok: false,
      problem: "not_found",
    });
    expect(await fetchInfo("t", undefined, answer(429, {}))).toEqual({
      ok: false,
      problem: "busy",
    });
    const f = answer(200, { device: "dev0", tuning: { mode: "follow" }, features: ["waterfall"] });
    const ok = await fetchInfo("t", undefined, f);
    expect(ok.ok).toBe(true);
    expect(f.mock.calls[0]?.[1]).toMatchObject({ credentials: "omit" });
  });
});

describe("share page", () => {
  const info = {
    v: 1,
    title: "Hilltop",
    description: "A roof antenna",
    location: "Hill",
    device: "dev0",
    features: ["waterfall", "audio"],
    tuning: { mode: "follow" },
    listeners: 1,
    max_listeners: 5,
  };

  async function mount(embed: boolean, status = 200, body: unknown = info) {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((url: string) =>
        Promise.resolve(
          new Response(JSON.stringify(url.endsWith("/state") ? { vfos: [] } : body), {
            status,
          }),
        ),
      ),
    );
    const { ShareApp } = await import("./ShareApp");
    render(<ShareApp token="tok" embed={embed} />);
    await act(() => new Promise<void>((r) => setTimeout(r, 0)));
  }

  it("shows the station, the waterfall and a play button that has not started anything", async () => {
    await mount(false);
    expect(await screen.findByRole("heading", { name: "Hilltop" })).toBeInTheDocument();
    expect(screen.getByText("A roof antenna")).toBeInTheDocument();
    expect(screen.getByTestId("spectrum")).toBeInTheDocument();
    // No audio socket until the visitor clicks: only REST requests were made.
    const urls = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.map((c) => String(c[0]));
    expect(urls.every((u) => u.startsWith("/api/v1/share/tok"))).toBe(true);
    expect(screen.getByRole("button", { name: "Play" })).toBeEnabled();
    expect(screen.getByText("Powered by Superhet")).toBeInTheDocument();
  });

  it("the embed variant is compact: no description or footer", async () => {
    await mount(true);
    expect(await screen.findByRole("heading", { name: "Hilltop" })).toBeInTheDocument();
    expect(screen.queryByText("A roof antenna")).toBeNull();
    expect(screen.queryByText("Powered by Superhet")).toBeNull();
    expect(screen.getByRole("button", { name: "Play" })).toBeInTheDocument();
  });

  it("says why a link does not work", async () => {
    await mount(false, 410, { error: "revoked" });
    expect(await screen.findByText("This link was withdrawn by the station.")).toBeInTheDocument();
  });
});
