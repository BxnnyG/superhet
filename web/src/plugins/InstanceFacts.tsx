import { cn } from "../ui";
import type { PluginInstance } from "./api";
import { Fact } from "./parts";
import { formatBytes, formatLevel, lastDecodeText, type Flow } from "./liveness";

interface Props {
  instance: PluginInstance;
  flow: Flow;
  /** What the decoder produces ("packets", "pages"); default "events". */
  noun?: string;
  className?: string;
}

/**
 * The counters that tell "running but nothing decoded" from "no input": what arrived (bytes,
 * level), what came out (events, last time), what was lost (dropped input, invalid lines).
 */
export function InstanceFacts({ instance: i, flow, noun = "events", className }: Props) {
  const last = lastDecodeText(i);
  const hasInput = i.vfo !== null;
  return (
    <div
      role="group"
      aria-label="Decoder counters"
      className={cn("m-0 flex flex-wrap items-baseline gap-x-4 gap-y-1", className)}
    >
      {hasInput && (
        <Fact label="Input">
          <span
            data-testid="facts-input"
            className={flow === "stalled" || i.inputBytes === 0 ? "text-warn" : undefined}
          >
            {i.inputBytes === 0
              ? "none yet"
              : `${formatBytes(i.inputBytes)}${i.inputLevelDb === null ? "" : ` · ${formatLevel(i.inputLevelDb)}`}`}
          </span>
        </Fact>
      )}
      <Fact label="Decoded">
        <span data-testid="facts-decoded">
          {i.eventsOk} {noun}
        </span>
      </Fact>
      <Fact label="Last">
        <span data-testid="facts-last">{last ?? "never"}</span>
      </Fact>
      {i.inputDropped > 0 && (
        <Fact label="Dropped">
          <span className="text-warn" title="Input chunks the decoder was too slow to take">
            {i.inputDropped} chunks
          </span>
        </Fact>
      )}
      {i.eventsInvalid > 0 && (
        <Fact label="Invalid">
          <span className="text-warn" title="Output lines of the decoder that were rejected">
            {i.eventsInvalid} lines
          </span>
        </Fact>
      )}
      {i.restarts > 0 && (
        <Fact label="Restarts">
          <span className="text-warn">{i.restarts}</span>
        </Fact>
      )}
    </div>
  );
}
