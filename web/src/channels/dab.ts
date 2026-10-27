// DAB blocks are not demodulated by a VFO: selecting one moves the device centre to the block
// centre and needs a sample rate whose usable window covers the 1.536 MHz ensemble.

import { USABLE_FRACTION } from "../spectrum/api";

/** Width of a DAB ensemble (one Band III block). */
export const DAB_BLOCK_HZ = 1_536_000;

export const DAB_PLUGIN_NOTICE = "DAB decoding needs the DAB plugin (coming in P2)";

/**
 * The sample rate to use for a DAB block: the current one when its usable window already covers
 * the block, else the smallest offered rate that does (the largest if none does).
 */
export function dabSampleRate(currentHz: number | null, offered: readonly number[]): number {
  const covers = (rate: number) => rate * USABLE_FRACTION >= DAB_BLOCK_HZ;
  if (currentHz !== null && covers(currentHz)) return currentHz;
  const sorted = [...offered].sort((a, b) => a - b);
  return (
    sorted.find(covers) ?? sorted[sorted.length - 1] ?? Math.ceil(DAB_BLOCK_HZ / USABLE_FRACTION)
  );
}
