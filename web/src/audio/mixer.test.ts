import { describe, expect, it } from "vitest";
import { Mixer } from "./mixer";

const FRAME = 960;

function prime(m: Mixer, id: number, value: number, frames = 6) {
  for (let i = 0; i < frames; i++) m.push(id, new Float32Array(FRAME).fill(value), i * FRAME);
}

describe("Mixer", () => {
  it("sums channels", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    m.add(2);
    prime(m, 1, 0.25);
    prime(m, 2, 0.5);
    const out = new Float32Array(128);
    m.render(out);
    expect(out[64]).toBeCloseTo(0.75, 3);
  });

  it("clips to [-1, 1]", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    m.add(2);
    prime(m, 1, 0.9);
    prime(m, 2, 0.9);
    const out = new Float32Array(128);
    m.render(out);
    expect(out[64]).toBe(1);
  });

  it("ramps to a new gain and mutes without a step", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    prime(m, 1, 0.5, 20);
    const out = new Float32Array(128);
    m.render(out);
    m.setGain(1, 0);
    m.render(out);
    expect(out[0]).toBeGreaterThan(0.4); // still ramping, no instant jump
    for (let i = 0; i < 30; i++) m.render(out);
    expect(Math.abs(out[127] ?? 1)).toBeLessThan(1e-3);
  });

  it("ignores unknown ids and removed channels", () => {
    const m = new Mixer({ outputRate: 48_000 });
    m.push(9, new Float32Array(FRAME), 0);
    m.add(1);
    m.remove(1);
    expect(m.has(1)).toBe(false);
    const out = new Float32Array(128).fill(1);
    m.render(out);
    expect(out.every((v) => v === 0)).toBe(true);
  });

  it("reports per-VFO stats", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(4);
    expect(m.stats()[0]).toMatchObject({ id: 4, state: "buffering", underruns: 0 });
  });
});

describe("Mixer stereo", () => {
  function primeStereo(m: Mixer, id: number, left: number, right: number, frames = 6) {
    for (let i = 0; i < frames; i++) {
      m.push(
        id,
        new Float32Array(FRAME).fill(left),
        i * FRAME,
        new Float32Array(FRAME).fill(right),
      );
    }
  }

  it("keeps the channels apart when rendering stereo", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    primeStereo(m, 1, 0.5, -0.25);
    const left = new Float32Array(128);
    const right = new Float32Array(128);
    m.render(left, right);
    expect(left[64]).toBeCloseTo(0.5, 3);
    expect(right[64]).toBeCloseTo(-0.25, 3);
  });

  it("feeds mono audio to both channels", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    prime(m, 1, 0.4);
    const left = new Float32Array(128);
    const right = new Float32Array(128);
    m.render(left, right);
    expect(left[64]).toBeCloseTo(0.4, 3);
    expect(right[64]).toBeCloseTo(0.4, 3);
  });

  it("downmixes stereo when only one output channel is asked for", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    primeStereo(m, 1, 0.5, 0.1);
    const out = new Float32Array(128);
    m.render(out);
    expect(out[64]).toBeCloseTo(0.3, 3);
  });

  it("mixes stereo and mono VFOs per channel", () => {
    const m = new Mixer({ outputRate: 48_000, targetMs: 100 });
    m.add(1);
    m.add(2);
    primeStereo(m, 1, 0.5, 0);
    prime(m, 2, 0.25);
    const left = new Float32Array(128);
    const right = new Float32Array(128);
    m.render(left, right);
    expect(left[64]).toBeCloseTo(0.75, 3);
    expect(right[64]).toBeCloseTo(0.25, 3);
  });
});
