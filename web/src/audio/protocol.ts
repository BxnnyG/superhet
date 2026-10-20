// Message types between the main thread and the AudioWorklet (see worklet.ts).

import type { JitterStats } from "./jitter";

export const WORKLET_NAME = "superhet-mixer";

export type ToWorklet =
  | { type: "config"; targetMs: number; inputRate: number }
  | { type: "add"; vfo: number; gain: number }
  | { type: "remove"; vfo: number }
  | { type: "gain"; vfo: number; gain: number }
  | { type: "target"; ms: number }
  | { type: "pcm"; vfo: number; pos: number; samples: Float32Array };

export interface FromWorklet {
  type: "stats";
  vfos: Array<{ id: number } & JitterStats>;
}
