import { renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { formatBytes, formatLevel, useFlow, verdict } from "./liveness";
import { instance } from "./testing";

describe("verdict", () => {
  it("tells no input from nothing decoded", () => {
    const quiet = instance({ events_ok: 0, last_event_unix_ms: null });
    expect(verdict(quiet, "flowing")).toMatchObject({
      tone: "success",
      text: expect.stringContaining("nothing decoded yet"),
    });
    expect(verdict(instance({ input_bytes: 0, events_ok: 0 }), "unknown")).toMatchObject({
      tone: "warn",
      text: expect.stringContaining("No samples"),
    });
    expect(verdict(quiet, "stalled")).toMatchObject({
      tone: "warn",
      text: expect.stringContaining("input stopped"),
    });
    expect(verdict(instance({ input_error: "VFO 1 does not exist" }), "unknown").text).toBe(
      "No input: VFO 1 does not exist",
    );
    expect(verdict(instance(), "flowing").text).toBe("Decoding.");
  });

  it("reports the lifecycle states first", () => {
    expect(verdict(instance({ state: "starting" }), "unknown").text).toMatch(/Starting/);
    expect(verdict(instance({ state: "backoff" }), "unknown").tone).toBe("warn");
    expect(verdict(instance({ state: "error" }), "unknown").tone).toBe("danger");
  });

  it("does not demand input from a plugin without a VFO", () => {
    const noVfo = instance({ vfo: null, input_bytes: 0, events_ok: 0 });
    expect(verdict(noVfo, "unknown").tone).toBe("success");
  });
});

describe("formatting", () => {
  it("formats levels and sizes", () => {
    expect(formatLevel(-32.4)).toBe("-32 dBFS");
    expect(formatLevel(-130)).toBe("<-120 dBFS");
    expect(formatLevel(null)).toBe("no signal data");
    expect(formatBytes(900)).toBe("900 B");
    expect(formatBytes(441_000)).toBe("441 kB");
    expect(formatBytes(5_300_000)).toBe("5.3 MB");
  });
});

describe("useFlow", () => {
  it("is flowing while the byte counter grows and stalled when it stands still", () => {
    const { result, rerender } = renderHook(({ i }) => useFlow(i), {
      initialProps: { i: instance({ input_bytes: 1000 }) },
    });
    expect(result.current).toBe("flowing");
    rerender({ i: instance({ input_bytes: 5000 }) });
    expect(result.current).toBe("flowing");
    rerender({ i: instance({ input_bytes: 5000 }) });
    expect(result.current).toBe("stalled");
    rerender({ i: instance({ input_bytes: 9000 }) });
    expect(result.current).toBe("flowing");
  });

  it("starts unknown without input and forgets a removed instance", () => {
    const { result, rerender } = renderHook(({ i }) => useFlow(i), {
      initialProps: { i: instance({ input_bytes: 0 }) as ReturnType<typeof instance> | undefined },
    });
    expect(result.current).toBe("unknown");
    rerender({ i: undefined });
    expect(result.current).toBe("unknown");
  });
});
