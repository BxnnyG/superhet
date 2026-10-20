import { describe, expect, it } from "vitest";
import { JitterBuffer, MAX_RATIO_ADJ } from "./jitter";

const RATE = 48_000;
const FRAME = 960;

function ramp(start: number, n: number): Float32Array {
  const a = new Float32Array(n);
  for (let i = 0; i < n; i++) a[i] = (start + i) / 1e6;
  return a;
}

function sine(startSample: number, n: number, hz = 440): Float32Array {
  const a = new Float32Array(n);
  for (let i = 0; i < n; i++) a[i] = 0.5 * Math.sin((2 * Math.PI * hz * (startSample + i)) / RATE);
  return a;
}

describe("JitterBuffer", () => {
  it("outputs silence until the target fill is reached, then plays", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    const out = new Float32Array(128).fill(9);
    jb.push(ramp(1, 2400), 0); // 50 ms
    jb.pull(out);
    expect(out.every((v) => v === 0)).toBe(true);
    expect(jb.stats().state).toBe("buffering");
    jb.push(ramp(2401, 2400), 2400); // 100 ms
    jb.pull(out);
    expect(jb.stats().state).toBe("playing");
    // ratio is 1 +- tiny drift correction: values follow the ramp
    expect(out[10]).toBeGreaterThan(0);
  });

  it("is transparent at the nominal rate when the fill sits on target", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.push(ramp(0, 4800 + 960), 0);
    const out = new Float32Array(128);
    jb.pull(out);
    for (let i = 4; i < 128; i++) expect(out[i]).toBeCloseTo(i / 1e6, 5);
  });

  it("underruns to silence and rebuffers to the target", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.push(ramp(0, 4800), 0);
    const out = new Float32Array(128);
    let pulls = 0;
    while (jb.stats().underruns === 0 && pulls++ < 100) jb.pull(out);
    expect(jb.stats().underruns).toBe(1);
    expect(jb.stats().state).toBe("buffering");
    expect(out[127]).toBe(0);
    // Small top-up is not enough, the target is.
    jb.push(ramp(4800, 960), 4800);
    jb.pull(out);
    expect(jb.stats().state).toBe("buffering");
    jb.push(ramp(5760, 4800), 5760);
    jb.pull(out);
    expect(jb.stats().state).toBe("playing");
  });

  it("drops to the target on overrun", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    let pos = 0;
    const push = () => {
      jb.push(new Float32Array(FRAME), pos);
      pos += FRAME;
    };
    for (let i = 0; i < 5; i++) push(); // 100 ms
    jb.pull(new Float32Array(128)); // start playing
    for (let i = 0; i < 12; i++) push(); // flood
    const s = jb.stats();
    expect(s.overruns).toBeGreaterThan(0);
    expect(s.fillMs).toBeLessThanOrEqual(300 + 1);
    expect(s.fillMs).toBeGreaterThanOrEqual(100 - 1);
  });

  it("replaces lost frames by silence and counts them", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.push(new Float32Array(FRAME).fill(0.5), 0);
    jb.push(new Float32Array(FRAME).fill(0.5), 3 * FRAME); // 2 frames lost
    expect(jb.fill()).toBe(4 * FRAME);
    expect(jb.stats().gapSamples).toBe(2 * FRAME);
  });

  it("does not stretch time for a huge discontinuity", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.push(new Float32Array(FRAME), 0);
    jb.push(new Float32Array(FRAME), 10 * RATE);
    expect(jb.fill()).toBe(2 * FRAME);
    expect(jb.stats().gapSamples).toBe(0);
  });

  it("drops duplicate and overlapping data", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.push(new Float32Array(FRAME), 0);
    jb.push(new Float32Array(FRAME), 0);
    expect(jb.fill()).toBe(FRAME);
    jb.push(new Float32Array(FRAME), 480); // half overlaps
    expect(jb.fill()).toBe(FRAME + 480);
  });

  it("resamples to a different hardware rate", () => {
    const out = 44_100;
    const jb = new JitterBuffer({ outputRate: out, targetMs: 100 });
    jb.push(sine(0, 4800 + 9600), 0);
    const block = new Float32Array(441);
    const before = jb.fill();
    for (let i = 0; i < 10; i++) jb.pull(block); // 100 ms of output
    // ~4800 input samples consumed (within the drift correction)
    expect(before - jb.fill()).toBeGreaterThan(4800 * 0.99);
    expect(before - jb.fill()).toBeLessThan(4800 * 1.01);
  });

  it("shrinks the target to the capacity limit", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 5000, capacityMs: 1000 });
    expect(jb.stats().targetMs).toBeLessThan(340);
  });

  it("follows target changes", () => {
    const jb = new JitterBuffer({ outputRate: RATE, targetMs: 100 });
    jb.setTargetMs(250);
    expect(jb.stats().targetMs).toBeCloseTo(250, 0);
  });
});

/** Event-driven simulation: producer at nominal 48 kHz, consumer clock off by `ppm`. */
function simulate(opts: { ppm: number; seconds: number; targetMs?: number; jitterMs?: number }) {
  const jb = new JitterBuffer({ outputRate: RATE, targetMs: opts.targetMs ?? 100 });
  const consumerRate = RATE * (1 + opts.ppm * 1e-6);
  const block = 128;
  const out = new Float32Array(block);
  let nextPush = 0;
  let nextPull = 0;
  let pos = 0;
  let produced = 0;
  let played = 0;
  let maxDelta = 0;
  let last = 0;
  let playing = false;
  const fills: Array<{ t: number; fillMs: number }> = [];
  let maxAdj = 0;
  const lateAdj: number[] = [];
  while (Math.min(nextPush, nextPull) < opts.seconds) {
    if (nextPush <= nextPull) {
      // Deterministic arrival jitter of +-jitterMs around the nominal 20 ms grid.
      const j = (opts.jitterMs ?? 0) * Math.sin(produced * 1.7) * 1e-3;
      jb.push(sine(pos, FRAME), pos);
      pos += FRAME;
      produced += 1;
      nextPush = produced * 0.02 + j;
    } else {
      jb.pull(out);
      const s = jb.stats();
      maxAdj = Math.max(maxAdj, Math.abs(s.adj));
      if (nextPull > opts.seconds * 0.75) lateAdj.push(s.adj);
      if (s.state === "playing") {
        if (playing) {
          for (const v of out) {
            maxDelta = Math.max(maxDelta, Math.abs(v - last));
            last = v;
          }
        } else {
          last = out[block - 1] ?? 0;
        }
        playing = true;
      } else {
        playing = false;
      }
      played += block;
      nextPull += block / consumerRate;
      if (played % (block * 100) === 0) fills.push({ t: nextPull, fillMs: s.fillMs });
    }
  }
  const meanAdj = lateAdj.reduce((a, b) => a + b, 0) / Math.max(1, lateAdj.length);
  return { jb, fills, maxDelta, maxAdj, meanAdj };
}

describe("drift control", () => {
  it.each([-200, -50, 0, 50, 200])("holds the buffer near target at %i ppm", (ppm) => {
    const { jb, fills, maxAdj, meanAdj } = simulate({ ppm, seconds: 120 });
    const s = jb.stats();
    expect(s.underruns).toBe(0);
    expect(s.overruns).toBe(0);
    expect(maxAdj).toBeLessThanOrEqual(MAX_RATIO_ADJ + 1e-12);
    // After the start-up transient the fill level stays within +-25 ms of the 100 ms target.
    for (const f of fills.filter((x) => x.t > 40))
      expect(Math.abs(f.fillMs - 100)).toBeLessThan(25);
    // A faster sound card (+ppm) is compensated by a slower read ratio: correction ~ -ppm.
    expect(Math.abs(meanAdj * 1e6 + ppm)).toBeLessThan(50);
  });

  it("copes with arrival jitter without glitches", () => {
    const { jb, maxDelta } = simulate({ ppm: 150, seconds: 60, jitterMs: 8 });
    expect(jb.stats().underruns).toBe(0);
    // A 440 Hz sine of amplitude 0.5 moves at most ~0.029 per sample; no steps above that.
    expect(maxDelta).toBeLessThan(0.04);
  });

  it("stays smooth while the playback ratio is being adjusted", () => {
    const { maxDelta } = simulate({ ppm: 200, seconds: 30 });
    expect(maxDelta).toBeLessThan(0.04);
  });
});
