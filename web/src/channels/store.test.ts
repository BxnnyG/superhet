import { describe, expect, it, vi } from "vitest";
import { createChannelPlanApi, parsePlanList, serverId, type ChannelPlanApi } from "./api";
import { DAB_BLOCK_HZ, dabSampleRate } from "./dab";
import { planFromFile } from "./import";
import { DEFAULT_PREFS, effectiveRegion, loadPrefs, savePrefs } from "./prefs";
import { browserRegion, regionFromLocale, regionMatches, regionTokens } from "./regions";
import { ChannelStore, orderPlans } from "./store";
import { builtinPlans } from "./catalog";
import { parsePlan } from "./schema";

const doc = (name: string, over: Record<string, unknown> = {}) => ({
  v: 1,
  name,
  regions: ["DE"],
  channels: [{ number: "1", freq_hz: 145_500_000 }],
  ...over,
});

describe("regions", () => {
  it("knows which groups a country belongs to", () => {
    expect([...regionTokens("DE")].sort()).toEqual(["CEPT", "DE", "EU", "ITU-R1", "WORLD"].sort());
    expect(regionTokens("US").has("ITU-R2")).toBe(true);
    expect(regionTokens("GB").has("EU")).toBe(false);
    expect(regionTokens("GB").has("CEPT")).toBe(true);
    expect([...regionTokens(null)]).toEqual(["WORLD"]);
  });

  it("matches plans by code, group or WORLD", () => {
    expect(regionMatches(["DE"], "DE")).toBe(true);
    expect(regionMatches(["DE"], "AT")).toBe(false);
    expect(regionMatches(["CEPT"], "ch")).toBe(true);
    expect(regionMatches(["CEPT"], "US")).toBe(false);
    expect(regionMatches(["WORLD"], null)).toBe(true);
    expect(regionMatches(["EU"], null)).toBe(false);
  });

  it("takes the region from the browser locale", () => {
    expect(regionFromLocale("de-DE")).toBe("DE");
    expect(regionFromLocale("en-GB")).toBe("GB");
    expect(regionFromLocale("en")).toBeNull();
    expect(regionFromLocale("")).toBeNull();
    expect(regionFromLocale("not a locale!")).toBeNull();
    expect(browserRegion(["en", "fr-CH"])).toBe("CH");
    expect(browserRegion([])).toBeNull();
  });

  it("sorts the plans of the region first", () => {
    const ids = orderPlans(builtinPlans(), "DE").map((p) => p.id);
    expect(ids.indexOf("freenet-de")).toBeLessThan(ids.indexOf("pmr446") + 100);
    const us = orderPlans(builtinPlans(), "US").map((p) => p.id);
    // Worldwide plans first for the US, the CEPT / German ones below.
    expect(us.indexOf("marine-vhf")).toBeLessThan(us.indexOf("pmr446"));
    expect(us.indexOf("airband")).toBeLessThan(us.indexOf("freenet-de"));
    expect(new Set(us).size).toBe(builtinPlans().length);
  });
});

describe("preferences", () => {
  it("defaults, and ignores garbage", () => {
    const get = (v: string | null) => ({ getItem: () => v });
    expect(loadPrefs(get(null))).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(get("{"))).toEqual(DEFAULT_PREFS);
    expect(loadPrefs(get('{"region":"xx","stepByChannel":3,"rasters":{"a":1,"b":"c"}}'))).toEqual({
      region: null,
      stepByChannel: true,
      rasters: { b: "c" },
    });
  });

  it("round-trips and follows the browser unless a region is chosen", () => {
    const mem = new Map<string, string>();
    const prefs = { region: "AT", stepByChannel: false, rasters: { airband: "8.33k" } };
    savePrefs(prefs, { setItem: (k, v) => void mem.set(k, v) });
    expect(loadPrefs({ getItem: (k) => mem.get(k) ?? null })).toEqual(prefs);
    expect(effectiveRegion(prefs, "DE")).toBe("AT");
    expect(effectiveRegion(DEFAULT_PREFS, "DE")).toBe("DE");
    expect(effectiveRegion(DEFAULT_PREFS, null)).toBeNull();
  });
});

describe("custom plans from the server", () => {
  it("reads valid plans under their server id and counts the ones it cannot read", () => {
    const { plans, skipped } = parsePlanList({
      v: 1,
      plans: [
        { id: "u1", name: "A", doc: doc("A") },
        { id: "u2", name: "B", doc: { v: 9 } },
        { id: "u3", name: "C", doc: doc("C", { channels: [] }) },
        "junk",
      ],
    });
    expect(plans.map((p) => p.id)).toEqual(["custom:u1"]);
    expect(skipped).toBe(3);
    expect(serverId(plans[0]!)).toBe("u1");
    expect(serverId(builtinPlans()[0]!)).toBeNull();
    expect(() => parsePlanList({ v: 1 })).toThrow();
  });

  it("sends the documents to the right routes", async () => {
    const fetchImpl = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) =>
      init?.method === "DELETE" || init?.method === "PUT" || init?.method === "POST"
        ? new Response(init.method === "DELETE" ? null : "{}", {
            status: init.method === "DELETE" ? 204 : 200,
          })
        : new Response(JSON.stringify({ v: 1, plans: [] }), { status: 200 }),
    ) as unknown as typeof fetch;
    const api = createChannelPlanApi(fetchImpl);
    await api.list();
    await api.create({ v: 1 });
    await api.update("a/b", { v: 1 });
    await api.remove("x");
    const calls = (fetchImpl as unknown as ReturnType<typeof vi.fn>).mock.calls.map(
      (c) => `${(c[1] as RequestInit | undefined)?.method ?? "GET"} ${String(c[0])}`,
    );
    expect(calls).toEqual([
      "GET /api/v1/channel-plans",
      "POST /api/v1/channel-plans",
      "PUT /api/v1/channel-plans/a%2Fb",
      "DELETE /api/v1/channel-plans/x",
    ]);
  });

  it("turns server errors into a message", async () => {
    const fetchImpl = (async () =>
      new Response(JSON.stringify({ v: 1, ok: false, code: "limit", error: "too many" }), {
        status: 409,
      })) as unknown as typeof fetch;
    await expect(createChannelPlanApi(fetchImpl).create({})).rejects.toThrow(/too many|409/);
  });
});

describe("ChannelStore", () => {
  const fakeApi = (initial: unknown[] = []): ChannelPlanApi & { docs: unknown[] } => {
    const docs = [...initial];
    return {
      docs,
      list: async () => ({
        plans: docs.map((d, i) => parsePlan(d, { id: `custom:${i}` })),
        skipped: 0,
      }),
      create: async (d) => void docs.push(d),
      update: async () => undefined,
      remove: async () => void docs.pop(),
    };
  };
  const make = (api: ChannelPlanApi, detected: string | null = "DE") =>
    new ChannelStore(api, DEFAULT_PREFS, detected, () => undefined);

  it("starts with the built-in plans in the region's order", () => {
    const s = make(fakeApi()).getSnapshot();
    expect(s.region).toBe("DE");
    expect(s.plans.length).toBe(builtinPlans().length);
    expect(s.loaded).toBe(false);
  });

  it("adds custom plans after a refresh and lets them be removed", async () => {
    const api = fakeApi();
    const store = make(api);
    expect(await store.add(doc("Club"))).toBeNull();
    const snap = store.getSnapshot();
    expect(snap.custom.map((p) => p.name)).toEqual(["Club"]);
    expect(snap.plans.length).toBe(builtinPlans().length + 1);
    // Found by the lookup index, like the built-in ones.
    expect(snap.plans.find((p) => p.name === "Club")?.builtin).toBe(false);
    const club = snap.custom[0]!;
    expect(await store.remove(club)).toBeNull();
    expect(store.getSnapshot().custom).toEqual([]);
    expect(await store.remove(builtinPlans()[0]!)).toMatch(/built-in/);
  });

  it("reports API failures and keeps the built-in plans", async () => {
    const api: ChannelPlanApi = {
      list: async () => {
        throw new Error("no database");
      },
      create: async () => {
        throw new Error("denied");
      },
      update: async () => undefined,
      remove: async () => undefined,
    };
    const store = make(api);
    await store.refresh();
    expect(store.getSnapshot().error).toBe("no database");
    expect(store.getSnapshot().loaded).toBe(true);
    expect(await store.add(doc("X"))).toBe("denied");
    expect(store.getSnapshot().plans.length).toBe(builtinPlans().length);
  });

  it("changes region, stepping and raster, notifies and persists", () => {
    const saved = vi.fn();
    const store = new ChannelStore(fakeApi(), DEFAULT_PREFS, "DE", saved);
    const seen = vi.fn();
    store.subscribe(seen);
    store.setRegion("US");
    expect(store.getSnapshot().region).toBe("US");
    store.setRegion(null);
    expect(store.getSnapshot().region).toBe("DE");
    store.setStepByChannel(false);
    store.setRaster("airband", "8.33k");
    const prefs = store.getSnapshot().prefs;
    expect(prefs.stepByChannel).toBe(false);
    expect(prefs.rasters).toEqual({ airband: "8.33k" });
    expect(saved).toHaveBeenCalledTimes(4);
    expect(seen).toHaveBeenCalledTimes(4);
  });
});

describe("importing a file", () => {
  it("reads a plan JSON and tells what is wrong with a bad one", () => {
    const p = planFromFile(JSON.stringify(doc("Club")), "club.json", "DE");
    expect(p.name).toBe("Club");
    expect(() => planFromFile("{", "x.json", null)).toThrow(/JSON/);
    expect(() => planFromFile(JSON.stringify({ v: 1, name: "x" }), "x.json", null)).toThrow(
      /channel/,
    );
    expect(() => planFromFile("  ", "x.json", null)).toThrow(/empty/);
    expect(() => planFromFile("nothing useful", "x.csv", null)).toThrow(/no channels/);
  });

  it("turns a CHIRP CSV into a plan named after the file", () => {
    const csv = [
      "Location,Name,Frequency,Duplex,Offset,Tone,rToneFreq,cToneFreq,DtcsCode,DtcsPolarity,Mode,TStep,Skip,Comment",
      "0,Call,145.500000,,0.6,TSQL,88.5,88.5,023,NN,FM,12.5,,simplex",
      "1,Relais,438.700000,-,7.6,,88.5,88.5,023,NN,NFM,25,,",
    ].join("\n");
    const p = planFromFile(csv, "Club list.csv", "AT");
    expect(p.name).toBe("Club list");
    expect(p.regions).toEqual(["AT"]);
    expect(p.channels.map((c) => [c.number, c.name, c.freqHz, c.ctcssHz])).toEqual([
      ["1", "Call", 145_500_000, 88.5],
      ["2", "Relais", 438_700_000, null],
    ]);
    expect(planFromFile(csv, "x.csv", null).regions).toEqual(["WORLD"]);
  });
});

describe("DAB sample rate", () => {
  const rates = [250_000, 1_024_000, 1_400_000, 1_800_000, 2_048_000, 2_400_000, 3_200_000];

  it("keeps a rate whose usable window covers the block", () => {
    expect(dabSampleRate(2_400_000, rates)).toBe(2_400_000);
    expect(dabSampleRate(2_048_000, rates)).toBe(2_048_000);
  });

  it("widens a rate that is too small to the smallest offered one that covers", () => {
    // 85 % of 1.8 MS/s is 1.53 MHz: just short of 1.536.
    expect(dabSampleRate(1_800_000, rates)).toBe(2_048_000);
    expect(dabSampleRate(250_000, rates)).toBe(2_048_000);
    expect(dabSampleRate(null, rates)).toBe(2_048_000);
  });

  it("uses the largest rate when none is enough", () => {
    expect(dabSampleRate(1_000_000, [1_000_000, 1_400_000])).toBe(1_400_000);
    expect(dabSampleRate(null, [])).toBeGreaterThanOrEqual(DAB_BLOCK_HZ);
  });
});
