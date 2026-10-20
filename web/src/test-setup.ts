import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

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
