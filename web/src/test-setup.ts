import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

afterEach(cleanup);

// jsdom lacks these browser APIs; the spectrum view degrades gracefully without WebGL.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);

class WebSocketStub {
  static OPEN = 1;
  readyState = 0;
  binaryType = "blob";
  onopen = null;
  onclose = null;
  onmessage = null;
  onerror = null;
  send() {}
  close() {}
}
vi.stubGlobal("WebSocket", WebSocketStub);

HTMLCanvasElement.prototype.getContext = (() =>
  null) as typeof HTMLCanvasElement.prototype.getContext;
