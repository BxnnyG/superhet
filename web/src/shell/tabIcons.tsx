import type { ReactNode } from "react";
import {
  ActivityIcon,
  BookmarkIcon,
  GlobeIcon,
  ListIcon,
  PlaneIcon,
  SettingsIcon,
  SignalIcon,
  SlidersIcon,
} from "../ui";
import type { InspectorTab } from "./inspector";

/** Icon of each inspector tab (shared by the tab strip and the phone tab bar). */
export function tabIcon(tab: InspectorTab, size = 16): ReactNode {
  switch (tab) {
    case "receiver":
      return <SignalIcon size={size} />;
    case "bookmarks":
      return <BookmarkIcon size={size} />;
    case "channels":
      return <ListIcon size={size} />;
    case "decoders":
      return <ActivityIcon size={size} />;
    case "dab":
      return <GlobeIcon size={size} />;
    case "adsb":
      return <PlaneIcon size={size} />;
    case "display":
      return <SlidersIcon size={size} />;
    case "source":
      return <SettingsIcon size={size} />;
  }
}
