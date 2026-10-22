import { act, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../App";
import { DEFAULT_OVERLAY, OverlayStore } from "../bandplan/settings";
import { BookmarkStore } from "../bookmarks/store";
import { SpectrumMetrics } from "../shell/metrics";
import type { Vfo } from "../vfo/api";
import { VfoController } from "../vfo/controller";
import type { ApiClient, DeviceState } from "./api";
import { LevelController } from "./levels";
import { SpectrumView } from "./SpectrumView";
import { TuningController } from "./tuning";

// A 2D context whose every method is a no-op (enough for the plot to construct and draw).
function fake2d(): CanvasRenderingContext2D {
  return new Proxy({} as Record<string, unknown>, {
    get: (t, k: string) => (k in t ? t[k] : () => ({ width: 0 })),
    set: (t, k: string, v) => ((t[k] = v), true),
  }) as unknown as CanvasRenderingContext2D;
}

const original = HTMLCanvasElement.prototype.getContext;

afterEach(() => {
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
  Reflect.deleteProperty(HTMLElement.prototype, "clientHeight");
  HTMLCanvasElement.prototype.getContext = original;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("keeps the receiver running and shows a calm notice when WebGL2 is unavailable", () => {
  // 2D works, getContext("webgl2") returns null (WebGL disabled in the browser).
  HTMLCanvasElement.prototype.getContext = ((kind: string) =>
    kind === "2d" ? fake2d() : null) as typeof HTMLCanvasElement.prototype.getContext;
  const sockets: string[] = [];
  class SocketSpy {
    static OPEN = 1;
    readyState = 0;
    binaryType = "blob";
    onopen = null;
    onclose = null;
    onmessage = null;
    onerror = null;
    constructor(url: string) {
      sockets.push(url);
    }
    send() {}
    close() {}
  }
  vi.stubGlobal("WebSocket", SocketSpy);
  const fetchSpy = vi.fn().mockRejectedValue(new Error("offline"));
  vi.stubGlobal("fetch", fetchSpy);

  render(<App />);

  expect(screen.getByText(/Waterfall needs WebGL2/)).toBeVisible();
  // The spectrum socket and the tuning controller still started.
  expect(sockets.some((u) => u.includes("/spectrum"))).toBe(true);
  expect(fetchSpy.mock.calls.some(([u]) => String(u).includes("/state"))).toBe(true);
  // No blocking error banner for the missing WebGL.
  expect(screen.queryByText(/WebGL2 is not available/)).toBeNull();
});

function spectrumFrame(centerHz: number, sampleRate: number, bins = 64): ArrayBuffer {
  const buf = new ArrayBuffer(32 + bins);
  const v = new DataView(buf);
  v.setUint8(0, 1);
  v.setUint8(1, 0);
  v.setUint16(2, bins, true);
  v.setUint32(4, sampleRate, true);
  v.setBigUint64(8, BigInt(centerHz), true);
  v.setFloat32(24, -100, true);
  v.setFloat32(28, 0, true);
  return buf;
}

const VFO: Vfo = {
  id: 1,
  freqHz: 100_100_000,
  mode: "wfm",
  bandwidthHz: 200_000,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
};

function mountView(): { vfoCtl: VfoController; tuning: TuningController } {
  const tuning = new TuningController({
    device: "dev",
    api: {
      getState: () => new Promise<DeviceState>(() => undefined),
    } as unknown as ApiClient,
  });
  const vfoCtl = new VfoController({
    device: "dev",
    api: { list: () => new Promise<Vfo[]>(() => undefined) } as never,
  });
  render(
    <SpectrumView
      device="dev"
      controller={new LevelController()}
      tuning={tuning}
      vfoCtl={vfoCtl}
      step="auto"
      createVfoAt={() => undefined}
      overlaySettings={new OverlayStore(DEFAULT_OVERLAY, null)}
      bookmarks={new BookmarkStore({} as never)}
      onAnnotationTune={() => undefined}
      metrics={new SpectrumMetrics()}
      registerView={() => undefined}
      onStatus={() => undefined}
      onInfo={() => undefined}
      onError={() => undefined}
    />,
  );
  return { vfoCtl, tuning };
}

it("draws VFO overlays without WebGL2 and without hovering", async () => {
  // Record every 2D call per canvas; getContext("webgl2") stays null.
  const calls = new Map<HTMLCanvasElement, string[]>();
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string) {
    if (kind !== "2d") return null;
    const log: string[] = [];
    calls.set(this, log);
    return new Proxy({} as Record<string, unknown>, {
      get: (t, k: string) =>
        k in t
          ? t[k]
          : () => {
              log.push(k);
              return k === "measureText" ? { width: 10 } : undefined;
            },
      set: (t, k: string, v) => ((t[k] = v), true),
    });
  } as unknown as typeof HTMLCanvasElement.prototype.getContext;
  vi.stubGlobal("devicePixelRatio", 1);
  vi.stubGlobal(
    "Path2D",
    class {
      moveTo() {}
      lineTo() {}
      closePath() {}
    },
  );
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, value: 800 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, value: 400 });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
  const sockets: { onmessage: ((ev: MessageEvent) => void) | null; onopen: (() => void) | null }[] =
    [];
  class SocketSpy {
    static OPEN = 1;
    readyState = 1;
    binaryType = "blob";
    onopen: (() => void) | null = null;
    onclose = null;
    onmessage: ((ev: MessageEvent) => void) | null = null;
    onerror = null;
    constructor() {
      sockets.push(this);
    }
    send() {}
    close() {}
  }
  vi.stubGlobal("WebSocket", SocketSpy);

  const { vfoCtl } = mountView();
  const overlay = document.querySelector("canvas.cursor-crosshair") as HTMLCanvasElement;
  act(() => vfoCtl.ingest([VFO]));
  // The first redraw happens before any frame is known; then a frame arrives. The pointer never
  // enters the canvas.
  await new Promise((r) => setTimeout(r, 20));
  act(() => {
    sockets[0]?.onmessage?.({
      data: spectrumFrame(100_000_000, 2_400_000),
    } as MessageEvent);
  });
  await vi.waitFor(() => {
    const log = calls.get(overlay) ?? [];
    expect(log).toContain("fillRect"); // passband band
    expect(log).toContain("fillText"); // label chip
  });
  expect(document.body).toHaveTextContent(/Waterfall needs WebGL2/);
});
