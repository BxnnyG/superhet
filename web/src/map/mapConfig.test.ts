import { describe, expect, it } from "vitest";
import {
  attributionHtml,
  DEFAULT_MAP,
  loadMapConfig,
  parseMapConfig,
  tileHost,
  TileWatch,
} from "./mapConfig";

describe("parseMapConfig", () => {
  it("reads the server's source", () => {
    expect(
      parseMapConfig({
        tile_url: "http://tiles.lan/{z}/{x}/{y}.png",
        attribution: " Own ",
        attribution_url: "https://example.org",
      }),
    ).toEqual({
      tileUrl: "http://tiles.lan/{z}/{x}/{y}.png",
      proxied: false,
      attribution: "Own",
      attributionUrl: "https://example.org",
    });
  });

  it("accepts the server's own tile proxy path, but no protocol-relative URL", () => {
    const c = parseMapConfig({ tile_url: "/api/v1/map/tiles/{z}/{x}/{y}.png", proxied: true });
    expect(c.tileUrl).toBe("/api/v1/map/tiles/{z}/{x}/{y}.png");
    expect(c.proxied).toBe(true);
    expect(parseMapConfig({ tile_url: "//evil.example/{z}/{x}/{y}.png" })).toEqual(DEFAULT_MAP);
  });

  it("falls back to OpenStreetMap for anything unusable", () => {
    expect(parseMapConfig(null)).toEqual(DEFAULT_MAP);
    expect(parseMapConfig({})).toEqual(DEFAULT_MAP);
    expect(parseMapConfig({ tile_url: "javascript:alert(1)" })).toEqual(DEFAULT_MAP);
  });

  it("drops an attribution link that is not http(s)", () => {
    const c = parseMapConfig({
      tile_url: "https://t.example/{z}/{x}/{y}.png",
      attribution: "x",
      attribution_url: "javascript:alert(1)",
    });
    expect(c.attributionUrl).toBeNull();
  });
});

describe("loadMapConfig", () => {
  it("uses the default when the route is missing or the network fails", async () => {
    expect(await loadMapConfig(async () => new Response("", { status: 404 }))).toEqual(DEFAULT_MAP);
    expect(
      await loadMapConfig(async () => {
        throw new Error("offline");
      }),
    ).toEqual(DEFAULT_MAP);
  });

  it("returns what the server says", async () => {
    const res = () =>
      new Response(
        JSON.stringify({ tile_url: "https://t.example/{z}/{x}/{y}.png", attribution: "T" }),
      );
    expect((await loadMapConfig(async () => res())).tileUrl).toBe(
      "https://t.example/{z}/{x}/{y}.png",
    );
  });
});

describe("attributionHtml", () => {
  it("escapes text and link", () => {
    expect(
      attributionHtml({
        tileUrl: "",
        proxied: false,
        attribution: "<i>x</i>",
        attributionUrl: null,
      }),
    ).toBe("&#60;i&#62;x&#60;/i&#62;");
    const html = attributionHtml({
      tileUrl: "",
      proxied: false,
      attribution: "A",
      attributionUrl: 'https://e.org/?a="b"',
    });
    expect(html).toContain('rel="noopener noreferrer"');
    expect(html).not.toContain('"b"');
  });

  it("is empty without a text", () => {
    expect(
      attributionHtml({
        tileUrl: "",
        proxied: false,
        attribution: "",
        attributionUrl: "https://e.org",
      }),
    ).toBe("");
  });
});

describe("tileHost", () => {
  it("names the server behind a template", () => {
    expect(tileHost("https://tile.openstreetmap.org/{z}/{x}/{y}.png")).toBe(
      "tile.openstreetmap.org",
    );
    expect(tileHost("http://tiles.lan:8080/{z}/{x}/{y}.png")).toBe("tiles.lan:8080");
  });
});

describe("TileWatch", () => {
  it("complains after a few failures before any tile loaded", () => {
    const w = new TileWatch(3);
    expect([w.failure(), w.failure(), w.failure()]).toEqual([false, false, true]);
  });

  it("stays quiet once the server has served a tile", () => {
    const w = new TileWatch(2);
    w.loaded();
    expect([w.failure(), w.failure(), w.failure()]).toEqual([false, false, false]);
  });
});
