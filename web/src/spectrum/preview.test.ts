import { describe, expect, it } from "vitest";
import {
  GLIDE_MS,
  easeOut,
  glide,
  isOutside,
  previewLabel,
  prefersReducedMotion,
  snapClick,
} from "./preview";

describe("snapClick", () => {
  it("snaps to the step of the VFO, like the click does", () => {
    expect(snapClick(145_512_345, 12_500)).toBe(145_512_500);
    expect(snapClick(98_149_999, 100_000)).toBe(98_100_000);
    expect(snapClick(98_150_000, 100_000)).toBe(98_200_000);
  });

  it("never snaps below one step", () => {
    expect(snapClick(1_000, 25_000)).toBe(25_000);
  });

  it("rounds to whole Hz when tuning the centre", () => {
    expect(snapClick(433_920_000.6, null)).toBe(433_920_001);
  });

  it("ignores a degenerate step", () => {
    expect(snapClick(433_920_000.4, 0)).toBe(433_920_000);
  });
});

describe("previewLabel", () => {
  it("names the frequency and the effect", () => {
    expect(previewLabel(145_500_000, { kind: "vfo", id: 2 }, false)).toBe(
      "145.500 000 MHz  → VFO 2",
    );
    expect(previewLabel(98_100_000, { kind: "centre" }, false)).toBe("98.100 000 MHz  → centre");
    expect(previewLabel(7_100_000, { kind: "create" }, false)).toBe("7.100 000 MHz  → new VFO");
    expect(previewLabel(7_100_000, { kind: "grab", id: 1 }, false)).toBe("7.100 000 MHz  VFO 1");
  });

  it("flags positions outside the tuning window", () => {
    expect(previewLabel(100_000_000, { kind: "vfo", id: 1 }, true)).toContain("outside window");
    expect(previewLabel(100_000_000, { kind: "centre" }, true)).toContain("outside window");
  });
});

describe("isOutside", () => {
  const win = { startHz: 100e6, endHz: 102e6 };
  it("compares against the window edges", () => {
    expect(isOutside(win, 99.9e6)).toBe(true);
    expect(isOutside(win, 100e6)).toBe(false);
    expect(isOutside(win, 102e6)).toBe(false);
    expect(isOutside(win, 102.1e6)).toBe(true);
    expect(isOutside(null, 1)).toBe(false);
  });
});

describe("glide", () => {
  it("starts at the old position and ends at the new one", () => {
    expect(glide(100, 200, 0, false)).toBe(100);
    expect(glide(100, 200, GLIDE_MS, false)).toBe(200);
    expect(glide(100, 200, GLIDE_MS * 5, false)).toBe(200);
  });

  it("eases out monotonically", () => {
    let prev = 100;
    for (let t = 10; t < GLIDE_MS; t += 10) {
      const v = glide(100, 200, t, false);
      expect(v).toBeGreaterThan(prev);
      prev = v;
    }
    expect(glide(100, 200, GLIDE_MS / 2, false)).toBeGreaterThan(150);
  });

  it("jumps with reduced motion", () => {
    expect(glide(100, 200, 10, true)).toBe(200);
  });

  it("clamps easing input", () => {
    expect(easeOut(-1)).toBe(0);
    expect(easeOut(2)).toBe(1);
  });
});

describe("prefersReducedMotion", () => {
  it("is false without matchMedia", () => {
    expect(prefersReducedMotion()).toBe(false);
  });
});
