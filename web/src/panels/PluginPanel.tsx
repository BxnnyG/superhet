import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DecoderEvent } from "../decoders/events";
import type { PluginInstance } from "../plugins/api";
import { panelUrl, type PanelInfo } from "./api";
import {
  TuneLimiter,
  eventsMessage,
  fromPanelFrame,
  initMessage,
  instancesMessage,
  mayTune,
  panelEvents,
  panelInstances,
  parseGuestMessage,
  readTheme,
  themeMessage,
  tuneResultMessage,
} from "./bridge";

interface Props {
  panel: PanelInfo;
  instances: readonly PluginInstance[];
  /** Newest first. */
  events: readonly DecoderEvent[];
  /** Operators and admins (the manifest's `tune` flag is checked here as well). */
  isOperator: boolean;
  onTune: (freqHz: number) => void;
}

/**
 * One plugin panel in a sandboxed frame (ADR-0022). `sandbox="allow-scripts"` gives the page an
 * opaque origin: no cookies, no storage, no way to call the API as the viewer, no top navigation,
 * popups or forms. It talks to this component only through the versioned postMessage bridge in
 * `bridge.ts`: it gets a filtered copy of the instances and events of the plugins it declared,
 * the theme, and may ask for a tune request. A frame that navigates away is taken down.
 */
export function PluginPanel({ panel, instances, events, isOperator, onTune }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(false);
  const [loads, setLoads] = useState(0);
  const limiter = useMemo(() => new TuneLimiter(), []);
  const canTune = mayTune(panel.tune, isOperator);
  const tuneRef = useRef({ canTune, onTune });
  useEffect(() => {
    tuneRef.current = { canTune, onTune };
  });

  const post = useCallback((message: object) => {
    // The frame's origin is opaque: "*" is the only possible target origin. Only this frame's
    // window is addressed.
    frame.current?.contentWindow?.postMessage(message, "*");
  }, []);

  // Messages from the panel: only from this frame, only the closed vocabulary of the bridge.
  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      if (!fromPanelFrame(event, frame.current?.contentWindow)) return;
      const message = parseGuestMessage(event.data);
      if (message === null) return;
      if (message.type === "hello") {
        setReady(true);
        post(initMessage(panel.pluginId, tuneRef.current.canTune, readTheme()));
        return;
      }
      if (!tuneRef.current.canTune) {
        post(tuneResultMessage(false, "not allowed"));
      } else if (!limiter.allow()) {
        post(tuneResultMessage(false, "too many requests"));
      } else {
        tuneRef.current.onTune(message.freqHz);
        post(tuneResultMessage(true));
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [panel.pluginId, limiter, post]);

  // The data the panel may see, resent whenever it changes (after the panel said hello).
  const shownInstances = useMemo(
    () => panelInstances(instances, panel.reads),
    [instances, panel.reads],
  );
  const shownEvents = useMemo(() => panelEvents(events, panel.reads), [events, panel.reads]);
  useEffect(() => {
    if (ready) post(instancesMessage(shownInstances));
  }, [ready, shownInstances, post]);
  useEffect(() => {
    if (ready) post(eventsMessage(shownEvents));
  }, [ready, shownEvents, post]);

  // Theme changes (the user's choice or the system's) reach the panel too.
  useEffect(() => {
    if (!ready) return;
    const send = () => post(themeMessage(readTheme()));
    const observer = new MutationObserver(send);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    media?.addEventListener("change", send);
    return () => {
      observer.disconnect();
      media?.removeEventListener("change", send);
    };
  }, [ready, post]);

  // The page loads once. A second load means the panel navigated its own frame (the sandbox
  // cannot forbid that): the frame is removed so the bridge never talks to another page.
  if (loads > 1) {
    return (
      <p role="alert" className="m-4 text-sm text-danger">
        The panel “{panel.title}” navigated away and was closed.
      </p>
    );
  }
  return (
    <iframe
      ref={frame}
      title={panel.title}
      src={panelUrl(panel)}
      sandbox="allow-scripts"
      allow=""
      referrerPolicy="no-referrer"
      onLoad={() => setLoads((n) => n + 1)}
      className="h-full min-h-0 w-full flex-1 border-0 bg-surface-1"
    />
  );
}
