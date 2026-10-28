import { usePermissions } from "../auth/context";
import { PlainButton, cn } from "../ui";
import { INSPECTOR_LABEL, visibleTabs, type InspectorState, type InspectorTab } from "./inspector";
import { tabIcon } from "./tabIcons";
import { PHONE_QUERY, useMediaQuery } from "./useMediaQuery";

/** Tabs of the phone tab bar. Decoders (plugin picker, event log) are a tab of their own. */
const BAR_TABS: readonly InspectorTab[] = [
  "receiver",
  "bookmarks",
  "channels",
  "decoders",
  "dab",
  "adsb",
  "display",
  "source",
];

/**
 * Bottom tab bar below 600 px: each button opens (or closes) the inspector sheet on its tab.
 * Icon above a small label; the buttons share the width equally, so six fit on a 320 px phone.
 */
export function MobileTabBar({ inspector }: { inspector: InspectorState }) {
  const phoneUp = useMediaQuery(PHONE_QUERY);
  const visible = visibleTabs(usePermissions());
  if (phoneUp) return null;
  return (
    <nav
      aria-label="Panels"
      className="relative z-50 flex h-14 shrink-0 items-stretch border-t border-border bg-surface-1"
    >
      {BAR_TABS.filter((t) => visible.includes(t)).map((t) => {
        const active = inspector.open && inspector.tab === t;
        return (
          <PlainButton
            key={t}
            aria-pressed={active}
            aria-controls="inspector"
            onClick={() => (active ? inspector.close() : inspector.show(t))}
            className={cn(
              "flex min-h-11 min-w-0 flex-1 flex-col items-center justify-center gap-0.5 px-0.5",
              "text-[0.6875rem] leading-none font-medium transition-colors duration-150",
              active ? "bg-accent-soft text-accent" : "text-fg-muted hover:text-fg",
            )}
          >
            {tabIcon(t, 18)}
            <span className="max-w-full truncate">{INSPECTOR_LABEL[t]}</span>
          </PlainButton>
        );
      })}
    </nav>
  );
}
