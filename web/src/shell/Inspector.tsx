import { useRef, useState, type KeyboardEvent } from "react";
import { AudioControls } from "../audio/AudioControls";
import { usePermissions } from "../auth/context";
import { BandPlanPanel } from "../bandplan/BandPlanPanel";
import { BookmarksPanel } from "../bookmarks/BookmarksPanel";
import { ChannelsPanel } from "../channels/ChannelsPanel";
import { DecodersTab } from "../decoders/DecodersTab";
import { ProfilesPanel } from "../profiles/ProfilesPanel";
import { DisplayControls } from "../spectrum/DisplayControls";
import { CloseIcon, IconButton, PlainButton, Section, cn } from "../ui";
import { ReceiverPanel } from "../vfo/ReceiverPanel";
import { INSPECTOR_LABEL, visibleTabs, type InspectorState, type InspectorTab } from "./inspector";
import { SourceSettings } from "./SourceSettings";
import { PHONE_QUERY, useMediaQuery } from "./useMediaQuery";
import type { Receiver } from "./useReceiver";

const tabId = (tab: InspectorTab) => `inspector-tab-${tab}`;
const PANEL_ID = "inspector-panel";

interface Props {
  r: Receiver;
  inspector: InspectorState;
}

/**
 * Right-hand inspector with the tabs Receiver | Bookmarks | Decoders | Display | Source. Docked as
 * a 320 px column on wide screens (collapsible, `]`), a right sheet from 600 to 900 px and a
 * bottom sheet below 600 px, where the tab bar at the bottom picks the tab. Each tab body scrolls
 * on its own; the page never does.
 */
export function Inspector({ r, inspector }: Props) {
  const { open, docked, setTab, close } = inspector;
  const perms = usePermissions();
  const tabs = visibleTabs(perms);
  // A tab the role cannot see (guests: bookmarks) falls back to the receiver.
  const tab: InspectorTab = tabs.includes(inspector.tab) ? inspector.tab : "receiver";
  const phoneUp = useMediaQuery(PHONE_QUERY);
  const tabRefs = useRef<Partial<Record<InspectorTab, HTMLButtonElement | null>>>({});

  const onTabKey = (e: KeyboardEvent, index: number) => {
    const delta = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? tabs.length - 1
          : (index + delta + tabs.length) % tabs.length;
    if (delta === 0 && e.key !== "Home" && e.key !== "End") return;
    e.preventDefault();
    const target = tabs[next];
    if (!target) return;
    setTab(target);
    tabRefs.current[target]?.focus();
  };

  // Docked and hidden: nothing is mounted, the spectrum takes the width.
  if (docked && !open) return null;
  return (
    <>
      {open && !docked && (
        <div
          aria-hidden="true"
          onClick={close}
          className="fixed inset-0 z-30 bg-overlay shell:hidden"
        />
      )}
      <aside
        id="inspector"
        aria-label="Inspector"
        className={cn(
          "flex min-h-0 flex-col bg-surface-1",
          // Docked column.
          "shell:static shell:z-auto shell:w-80 shell:shrink-0 shell:border-l shell:border-border",
          // Right sheet (600-900 px).
          "max-shell:fixed max-shell:inset-y-0 max-shell:right-0 max-shell:z-40 max-shell:w-[23rem] max-shell:max-w-[92vw]",
          "max-shell:border-l max-shell:border-border max-shell:transition-[transform,visibility] max-shell:duration-150",
          // Bottom sheet above the tab bar (< 600 px).
          "max-phone:inset-x-0 max-phone:top-auto max-phone:bottom-14 max-phone:h-[65dvh] max-phone:w-auto max-phone:max-w-none",
          "max-phone:rounded-t-xl max-phone:border-t max-phone:border-l-0",
          open
            ? "max-shell:translate-x-0 max-shell:shadow-pop max-phone:translate-y-0"
            : "max-shell:invisible max-shell:translate-x-full max-phone:translate-x-0 max-phone:translate-y-full",
        )}
      >
        <div className="flex h-10 shrink-0 items-center border-b border-border pr-1 pl-1">
          {phoneUp ? (
            <div
              role="tablist"
              aria-label="Inspector"
              className="flex min-w-0 flex-1 items-stretch self-stretch overflow-x-auto px-1"
            >
              {tabs.map((t, i) => (
                <PlainButton
                  key={t}
                  ref={(el) => {
                    tabRefs.current[t] = el;
                  }}
                  role="tab"
                  id={tabId(t)}
                  aria-selected={tab === t}
                  aria-controls={PANEL_ID}
                  tabIndex={tab === t ? 0 : -1}
                  onClick={() => setTab(t)}
                  onKeyDown={(e) => onTabKey(e, i)}
                  className={cn(
                    "relative shrink-0 rounded-md px-1.5 text-xs font-medium transition-colors duration-150",
                    tab === t ? "text-fg" : "text-fg-subtle hover:text-fg",
                    // Underline of the selected tab.
                    tab === t &&
                      "after:absolute after:inset-x-1.5 after:bottom-0 after:h-0.5 after:rounded-full after:bg-accent",
                  )}
                >
                  {INSPECTOR_LABEL[t]}
                </PlainButton>
              ))}
            </div>
          ) : (
            <h2 className="flex-1 truncate pl-3 text-sm font-semibold">{INSPECTOR_LABEL[tab]}</h2>
          )}
          {!docked && (
            <IconButton
              label="Close inspector"
              size="sm"
              className="max-phone:size-11"
              onClick={close}
            >
              <CloseIcon />
            </IconButton>
          )}
        </div>
        <div
          id={PANEL_ID}
          role={phoneUp ? "tabpanel" : "region"}
          aria-labelledby={phoneUp ? tabId(tab) : undefined}
          aria-label={phoneUp ? undefined : INSPECTOR_LABEL[tab]}
          className={cn(
            "min-h-0 flex-1 overflow-x-hidden",
            // The Decoders tab pins its cards and scrolls the log itself.
            tab === "decoders" ? "overflow-y-hidden" : "overflow-y-auto",
          )}
        >
          {open && <TabBody r={r} tab={tab} phoneUp={phoneUp} />}
        </div>
      </aside>
    </>
  );
}

function TabBody({ r, tab, phoneUp }: { r: Receiver; tab: InspectorTab; phoneUp: boolean }) {
  const [profilesOpen, setProfilesOpen] = useState(true);
  const perms = usePermissions();
  switch (tab) {
    case "receiver":
      return (
        <ReceiverPanel
          vfo={r.selected}
          loaded={r.vfoSnap.loaded}
          audio={r.audio}
          audioSnap={r.audioSnap}
          autoSquelch={r.autoSquelch}
          locked={r.locks.length > 0}
          showDemodulator={!phoneUp}
          onUpdate={(id, patch) => r.vfoCtl.update(id, patch)}
          onRemove={(id) => r.vfoCtl.remove(id)}
        />
      );
    case "bookmarks":
      return (
        <div className="h-full min-h-0 px-4 pt-3">
          <BookmarksPanel
            store={r.bookmarks}
            overlay={r.overlay}
            activeFreqHz={r.selected?.freqHz ?? null}
            onTune={r.tuneTo}
            onTuneNew={r.tuneNew}
            onAdd={r.startBookmark}
          />
        </div>
      );
    case "channels":
      return <ChannelsPanel r={r} />;
    case "decoders":
      return (
        <DecodersTab
          vfos={r.vfoSnap.vfos}
          selectedId={r.selected?.id ?? null}
          audio={r.audio}
          audioSnap={r.audioSnap}
          events={r.decoderEvents}
          plugins={r.plugins}
          device={r.device}
          onUpdate={(id, patch) => r.vfoCtl.update(id, patch)}
          onTune={r.tuneTo}
        />
      );
    case "display":
      return (
        <>
          <Section title="Levels and zoom">
            <DisplayControls
              controller={r.levelController}
              info={r.info}
              viewActions={r.viewActions}
            />
          </Section>
          <Section title="Band plan">
            <BandPlanPanel store={r.overlay} />
          </Section>
        </>
      );
    case "source":
      return (
        <>
          <Section title="Source">
            <SourceSettings r={r} />
          </Section>
          {perms.canProfiles && (
            <Section title="Profiles" onOpenChange={setProfilesOpen}>
              <ProfilesPanel
                api={r.profileApi}
                device={r.device}
                doc={r.currentDoc}
                onState={r.applyDoc}
                onUnlock={r.unlock}
                connected={r.feed.status === "connected"}
                refreshSignal={String(profilesOpen)}
              />
            </Section>
          )}
          <Section title="Audio device">
            <AudioControls audio={r.audio} snapshot={r.audioSnap} />
          </Section>
        </>
      );
  }
}
