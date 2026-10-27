import { usePermissions } from "../auth/context";
import { PlainButton, cn } from "../ui";
import { INSPECTOR_LABEL, visibleTabs, type InspectorState, type InspectorTab } from "./inspector";
import { PHONE_QUERY, useMediaQuery } from "./useMediaQuery";

/** Tabs of the phone tab bar; Decoders live inside the Receiver tab there. */
const BAR_TABS: readonly InspectorTab[] = [
  "receiver",
  "bookmarks",
  "channels",
  "display",
  "source",
];

/** Bottom tab bar below 600 px: each button opens (or closes) the inspector sheet on its tab. */
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
              "min-h-11 flex-1 text-xs font-medium transition-colors duration-150",
              active ? "bg-accent-soft text-accent" : "text-fg-muted hover:text-fg",
            )}
          >
            {INSPECTOR_LABEL[t]}
          </PlainButton>
        );
      })}
    </nav>
  );
}
