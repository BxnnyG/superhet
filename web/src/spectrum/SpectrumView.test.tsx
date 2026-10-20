import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../App";

// A 2D context whose every method is a no-op (enough for the plot to construct and draw).
function fake2d(): CanvasRenderingContext2D {
  return new Proxy({} as Record<string, unknown>, {
    get: (t, k: string) => (k in t ? t[k] : () => ({ width: 0 })),
    set: (t, k: string, v) => ((t[k] = v), true),
  }) as unknown as CanvasRenderingContext2D;
}

const original = HTMLCanvasElement.prototype.getContext;

afterEach(() => {
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
