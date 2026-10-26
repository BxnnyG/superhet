// What the command palette offers and how a query ranks it. Pure: the effects come in as
// callbacks (PaletteDeps), so the search can be tested without the receiver.

import {
  bookmarkColor,
  bookmarkTarget,
  digitalBadge,
  toneBadge,
  type Bookmark,
} from "../bookmarks/model";
import type { AnnotationTarget } from "../spectrum/annotations";
import { formatFrequency } from "../spectrum/freq";
import type { Vfo } from "../vfo/api";
import { MODE_LABEL } from "../vfo/model";
import { INSPECTOR_LABEL, INSPECTOR_TABS, type InspectorTab } from "../shell/inspector";
import { MODE_KEYS, shortcutKeys, type ShortcutAction } from "../shell/shortcuts";
import { rankScored } from "./fuzzy";
import { parseTuneCommand, type TuneCommand } from "./parse";
import type { RecentEntry } from "./recent";

export type PaletteGroup = "Tune" | "Bookmarks" | "Recent" | "VFOs" | "Actions";

export interface PaletteItem {
  id: string;
  group: PaletteGroup;
  title: string;
  subtitle?: string;
  /** Extra search text (synonyms, tags). */
  keywords: readonly string[];
  /** Key caps of the equivalent shortcut. */
  hint?: readonly string[];
  /** Dot colour (bookmarks). */
  color?: string;
  /** Alt+Enter opens a new VFO instead of retuning the active one. */
  newVfoAlt?: boolean;
  run: (opts: { newVfo: boolean }) => void;
}

export interface PaletteSection {
  group: PaletteGroup;
  items: PaletteItem[];
}

export interface PaletteDeps {
  vfos: readonly Vfo[];
  selectedId: number | null;
  bookmarks: readonly Bookmark[];
  recent: readonly RecentEntry[];
  audioEnabled: boolean;
  /** Tune the active VFO (or make one); `newVfo` always makes one. */
  tune: (target: AnnotationTarget, newVfo: boolean) => void;
  /** Remember a tuned frequency in the recent list. */
  remember: (entry: RecentEntry) => void;
  /** Run a keyboard shortcut action (same code path as the key). */
  act: (action: ShortcutAction) => void;
  selectVfo: (id: number) => void;
  showTab: (tab: InspectorTab) => void;
  toggleAudio: () => void;
  exportLog: () => void;
  openSettings: () => void;
}

/** Longest list per group while searching; the list is for picking, not browsing. */
const LIMIT: Record<PaletteGroup, number> = {
  Tune: 1,
  Bookmarks: 8,
  Recent: 5,
  VFOs: 5,
  Actions: 10,
};
/** Without a query: this many entries of each group. */
const IDLE_LIMIT: Record<PaletteGroup, number> = {
  Tune: 0,
  Bookmarks: 5,
  Recent: 5,
  VFOs: 0,
  Actions: 99,
};

const withMhz = (hz: number) => `${formatFrequency(hz)} MHz`;

/** "446.006 250 MHz · NFM · CTCSS 88.5" */
export function describeTune(t: TuneCommand): string {
  return [
    withMhz(t.freqHz),
    t.mode ? MODE_LABEL[t.mode] : null,
    t.toneHz !== undefined ? `CTCSS ${t.toneHz.toFixed(1)}` : null,
  ]
    .filter((p): p is string => p !== null)
    .join(" · ");
}

function targetOf(t: TuneCommand): AnnotationTarget {
  return {
    freqHz: t.freqHz,
    ...(t.mode ? { mode: t.mode } : {}),
    ...(t.toneHz !== undefined ? { toneSquelchHz: t.toneHz } : {}),
  };
}

function tuneItem(deps: PaletteDeps, t: TuneCommand, group: PaletteGroup, id: string): PaletteItem {
  return {
    id,
    group,
    title: group === "Tune" ? `Tune to ${describeTune(t)}` : describeTune(t),
    ...(group === "Tune"
      ? { subtitle: deps.vfos.length > 0 ? "Active VFO" : "Creates a VFO" }
      : {}),
    keywords: [`${t.freqHz / 1e6}`, describeTune(t)],
    newVfoAlt: true,
    run: ({ newVfo }) => {
      deps.tune(targetOf(t), newVfo);
      deps.remember({
        freqHz: t.freqHz,
        ...(t.mode ? { mode: t.mode } : {}),
        ...(t.toneHz !== undefined ? { toneHz: t.toneHz } : {}),
      });
    },
  };
}

function bookmarkItem(deps: PaletteDeps, b: Bookmark): PaletteItem {
  const tone = toneBadge(b);
  const digital = digitalBadge(b);
  const detail = [
    withMhz(b.freqHz),
    b.mode ? MODE_LABEL[b.mode] : null,
    tone === null ? null : tone.startsWith("DCS") ? tone : `CTCSS ${tone}`,
    digital,
    b.tags.length > 0 ? b.tags.map((t) => `#${t}`).join(" ") : null,
  ].filter((p): p is string => p !== null);
  return {
    id: `bookmark:${b.id}`,
    group: "Bookmarks",
    title: b.name,
    subtitle: detail.join(" · "),
    keywords: [
      b.tags.join(" "),
      `${b.freqHz / 1e6}`,
      (b.freqHz / 1e6).toFixed(3),
      b.mode ? MODE_LABEL[b.mode] : "",
      digital ?? "",
    ],
    color: bookmarkColor(b),
    newVfoAlt: true,
    run: ({ newVfo }) => {
      deps.tune(bookmarkTarget(b), newVfo);
      deps.remember({ freqHz: b.freqHz, ...(b.mode ? { mode: b.mode } : {}) });
    },
  };
}

function actionItems(deps: PaletteDeps): PaletteItem[] {
  const active = deps.vfos.find((v) => v.id === deps.selectedId) ?? null;
  const out: PaletteItem[] = [];
  const add = (
    id: string,
    title: string,
    keywords: string[],
    run: () => void,
    shortcut?: string,
    hint?: readonly string[],
  ) => {
    const keys = hint ?? (shortcut ? shortcutKeys(shortcut) : null);
    out.push({
      id: `action:${id}`,
      group: "Actions",
      title,
      keywords,
      ...(keys ? { hint: keys } : {}),
      run,
    });
  };
  const act = (a: ShortcutAction) => () => deps.act(a);

  add(
    "new-vfo",
    "New VFO at the display centre",
    ["add", "receiver", "create"],
    act({ type: "newVfo" }),
    "newVfo",
  );
  if (active) {
    add(
      "close-vfo",
      "Close the active VFO",
      ["remove", "delete", "receiver"],
      act({ type: "delete" }),
      "delete",
    );
    add(
      "focus-freq",
      "Type a frequency",
      ["focus", "enter", "tune", "readout"],
      act({ type: "focusFrequency" }),
      "focusFrequency",
    );
    MODE_KEYS.forEach((m, i) => {
      if (m.mode === active.mode) return;
      add(
        `mode-${m.mode}`,
        `Switch mode to ${m.label}`,
        ["demodulator", "modulation", m.mode],
        act({ type: "mode", mode: m.mode, label: m.label }),
        undefined,
        [String(i + 1)],
      );
    });
    add(
      "mute",
      "Mute / unmute the active VFO",
      ["listen", "audio", "silence"],
      act({ type: "listen" }),
      "listen",
    );
    add(
      "squelch-auto",
      "Squelch Auto on / off",
      ["noise", "threshold"],
      act({ type: "autoSquelch" }),
      "autoSquelch",
    );
  }
  add(
    "bookmark",
    "Add a bookmark here",
    ["save", "favourite", "store"],
    act({ type: "bookmark" }),
    "bookmark",
  );
  add(
    "audio",
    deps.audioEnabled ? "Turn audio off" : "Turn audio on",
    ["sound", "speaker", "mute", "output"],
    deps.toggleAudio,
  );
  for (const tab of INSPECTOR_TABS) {
    add(`tab-${tab}`, `Open the ${INSPECTOR_LABEL[tab]} tab`, ["inspector", "panel", tab], () =>
      deps.showTab(tab),
    );
  }
  add(
    "inspector",
    "Show / hide the inspector",
    ["panel", "sidebar"],
    act({ type: "inspector" }),
    "inspector",
  );
  add(
    "zoom-in",
    "Zoom the spectrum in",
    ["spectrum", "waterfall"],
    act({ type: "zoom", factor: 0.5 }),
    undefined,
    ["+"],
  );
  add(
    "zoom-out",
    "Zoom the spectrum out",
    ["spectrum", "waterfall"],
    act({ type: "zoom", factor: 2 }),
    undefined,
    ["−"],
  );
  add(
    "zoom-reset",
    "Reset the zoom",
    ["spectrum", "waterfall", "full"],
    act({ type: "zoomReset" }),
    "zoomReset",
  );
  add(
    "export-log",
    "Export the decoder log (CSV)",
    ["download", "events", "rds", "dtmf", "ctcss"],
    deps.exportLog,
  );
  add(
    "settings",
    "Open settings",
    ["preferences", "theme", "password", "account"],
    deps.openSettings,
  );
  add(
    "shortcuts",
    "Show keyboard shortcuts",
    ["keys", "help", "hotkeys"],
    act({ type: "help" }),
    "help",
  );
  return out;
}

function vfoItems(deps: PaletteDeps): PaletteItem[] {
  return deps.vfos.map((v, i) => ({
    id: `vfo:${v.id}`,
    group: "VFOs" as const,
    title: `Select VFO ${v.id}`,
    subtitle: `${withMhz(v.freqHz)} · ${MODE_LABEL[v.mode]}${v.id === deps.selectedId ? " · active" : ""}`,
    keywords: [`vfo ${v.id}`, `${v.freqHz / 1e6}`, MODE_LABEL[v.mode], "receiver"],
    ...(i < 9 ? { hint: ["Alt", String(i + 1)] } : {}),
    run: () => deps.selectVfo(v.id),
  }));
}

const GROUP_ORDER: readonly PaletteGroup[] = ["Tune", "Bookmarks", "Recent", "VFOs", "Actions"];

/**
 * The palette content for a query: a "Tune" entry first when the query is a frequency command,
 * then the matching bookmarks, recent frequencies, VFOs and actions. Groups are ordered by their
 * best match; without a query a short default list is shown (recent, a few bookmarks, actions).
 */
export function buildSections(deps: PaletteDeps, query: string): PaletteSection[] {
  const q = query.trim();
  const sections: { section: PaletteSection; top: number }[] = [];
  const tuneCmd = parseTuneCommand(q);
  if (tuneCmd) {
    sections.push({
      section: { group: "Tune", items: [tuneItem(deps, tuneCmd, "Tune", "tune")] },
      top: Infinity,
    });
  }

  const pools: Record<Exclude<PaletteGroup, "Tune">, PaletteItem[]> = {
    Bookmarks: (q === ""
      ? [...deps.bookmarks].filter((b) => b.favourite)
      : [...deps.bookmarks]
    ).map((b) => bookmarkItem(deps, b)),
    Recent: deps.recent.map((r, i) =>
      tuneItem(
        deps,
        {
          freqHz: r.freqHz,
          ...(r.mode ? { mode: r.mode } : {}),
          ...(r.toneHz !== undefined ? { toneHz: r.toneHz } : {}),
        },
        "Recent",
        `recent:${i}`,
      ),
    ),
    VFOs: q === "" ? [] : vfoItems(deps),
    Actions: actionItems(deps),
  };

  for (const group of GROUP_ORDER) {
    if (group === "Tune") continue;
    const ranked = rankScored(pools[group], q, (i) => [i.title, ...i.keywords, i.subtitle ?? ""]);
    const limit = q === "" ? IDLE_LIMIT[group] : LIMIT[group];
    const items = ranked.slice(0, limit).map((r) => r.item);
    if (items.length === 0) continue;
    // Actions rank a little lower than data of equal quality so a frequency query shows data first.
    const top = (ranked[0]?.score ?? 0) - (group === "Actions" ? 10 : 0);
    sections.push({ section: { group, items }, top });
  }

  if (q !== "") sections.sort((a, b) => b.top - a.top);
  return sections.map((s) => s.section);
}

/** The items of all sections in display order (what the arrow keys walk through). */
export function flatten(sections: readonly PaletteSection[]): PaletteItem[] {
  return sections.flatMap((s) => s.items);
}
