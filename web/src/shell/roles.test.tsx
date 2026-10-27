import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthContext } from "../auth/context";
import { GUEST_LIMITS, accountStatus, authValue, guestStatus } from "../auth/testing";
import { TuningControls } from "../spectrum/TuningControls";
import type { LockHolder } from "../spectrum/api";
import type { TuningSnapshot } from "../spectrum/tuning";
import { setLockIdentity } from "../state/locks";
import { toasts } from "../ui";
import { fake, device, vfo } from "./fixtures";
import { visibleTabs } from "./inspector";
import { MobileTabBar } from "./MobileTabBar";
import { TuningLock } from "./TuningLock";
import { TuningStrip } from "./TuningStrip";
import { VfoTabs } from "./VfoTabs";
import type { InspectorState } from "./inspector";
import type { AuthStatus, GuestLimits } from "../auth/api";

function as(
  status: AuthStatus,
  ui: ReactNode,
  limits: GuestLimits | null = status.kind === "guest" ? GUEST_LIMITS : null,
) {
  setLockIdentity(authValue(status, { limits }).permissions.holderId);
  return render(<AuthContext value={authValue(status, { limits })}>{ui}</AuthContext>);
}

beforeEach(() => toasts.clear());
afterEach(() => {
  toasts.clear();
  setLockIdentity(undefined);
});

const mine = (id: number) => vfo(id, { mine: true, shared: false });
const theirs = (id: number) => vfo(id, { mine: false, shared: false });
const shared = (id: number) => vfo(id, { mine: false, shared: true });

describe("tuning lock by role", () => {
  const alice: LockHolder = { kind: "user", id: "u:7", name: "alice", sinceMs: 1 };
  const bob: LockHolder = { kind: "user", id: "u:8", name: "bob", sinceMs: 2 };

  it("names the holder and lets an operator release only their own lock", () => {
    const onToggle = vi.fn();
    const { unmount } = as(
      accountStatus("operator", "alice", 7),
      <TuningLock holders={[alice]} onToggle={onToggle} />,
    );
    expect(screen.getByText("Locked by you")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Unlock tuning" }));
    expect(onToggle).toHaveBeenCalledWith(false);
    unmount();

    const other = vi.fn();
    as(accountStatus("operator", "alice", 7), <TuningLock holders={[bob]} onToggle={other} />);
    expect(screen.getByText("Locked by bob")).toBeInTheDocument();
    const button = screen.getByRole("button", { name: "Lock tuning" });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(other).not.toHaveBeenCalled();
  });

  it("an admin can force-release somebody else's lock", () => {
    const onToggle = vi.fn();
    as(accountStatus("admin", "root", 1), <TuningLock holders={[bob]} onToggle={onToggle} />);
    expect(screen.getByText("Locked by bob")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Force unlock tuning" }));
    expect(onToggle).toHaveBeenCalledWith(false, true);
  });

  it("listeners and guests cannot take the lock but see who holds it", () => {
    for (const status of [accountStatus("listener", "lena", 4), guestStatus()]) {
      const onToggle = vi.fn();
      const { unmount } = as(status, <TuningLock holders={[alice]} onToggle={onToggle} />);
      expect(screen.getByText("Locked by alice")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Lock tuning" })).toBeDisabled();
      unmount();
    }
  });

  it("a guest sees an anonymous holder (names are hidden by the server)", () => {
    const anon: LockHolder = { kind: "user", sinceMs: 1, id: "user" };
    as(guestStatus(), <TuningLock holders={[anon]} onToggle={() => undefined} />);
    expect(screen.getByText("Locked by another user")).toBeInTheDocument();
  });
});

describe("source settings", () => {
  const state = { ...device, locks: [] };
  const snap: TuningSnapshot = {
    state,
    display: state,
    busy: false,
    lockBusy: false,
    error: null,
  };

  it("are read-only for listeners", () => {
    as(accountStatus("listener"), <TuningControls tuning={snap} onTune={() => undefined} />);
    expect(screen.getByLabelText("Centre frequency")).toBeDisabled();
    expect(screen.getByLabelText("Sample rate")).toBeDisabled();
    expect(screen.getByLabelText("Frequency correction")).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Automatic gain" })).toBeDisabled();
    expect(screen.getByText(/Operators and admins only/)).toBeInTheDocument();
  });

  it("are editable for operators", () => {
    as(accountStatus("operator"), <TuningControls tuning={snap} onTune={() => undefined} />);
    expect(screen.getByLabelText("Centre frequency")).toBeEnabled();
    expect(screen.getByLabelText("Frequency correction")).toBeEnabled();
  });
});

describe("VFO tabs by ownership", () => {
  it("only own receivers can be closed; shared ones can be heard, foreign ones cannot", () => {
    const { r } = fake([mine(1), shared(2), theirs(3)], 1);
    as(accountStatus("listener"), <VfoTabs r={r} />);
    expect(screen.getByRole("button", { name: "Delete VFO 1" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete VFO 2" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete VFO 3" })).toBeNull();
    expect(screen.getByRole("button", { name: "Listen to VFO 1" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Listen to VFO 2" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Listen to VFO 3" })).toBeNull();
  });

  it("operators manage and hear every receiver", () => {
    const { r } = fake([theirs(1), theirs(2)], 1);
    as(accountStatus("operator"), <VfoTabs r={r} />);
    expect(screen.getByRole("button", { name: "Delete VFO 2" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Listen to VFO 2" })).toBeInTheDocument();
  });

  it("does not assume 8 VFOs per device or user", () => {
    const vfos = Array.from({ length: 12 }, (_, i) => mine(i + 1));
    const { r } = fake(vfos, 1);
    as(accountStatus("listener"), <VfoTabs r={r} />);
    expect(screen.getByRole("button", { name: "Add VFO" })).toBeEnabled();
  });

  it("a guest at the policy limit cannot add another receiver", () => {
    const { r } = fake([mine(1)], 1);
    as(guestStatus(), <VfoTabs r={r} />);
    const add = screen.getByRole("button", { name: "Add VFO" });
    expect(add).toBeDisabled();
  });

  it("a guest below the limit can", () => {
    const { r } = fake([theirs(1)], 1);
    as(guestStatus(), <VfoTabs r={r} />, { ...GUEST_LIMITS, maxVfos: 2 });
    expect(screen.getByRole("button", { name: "Add VFO" })).toBeEnabled();
  });
});

describe("tuning strip", () => {
  it("shows somebody else's receiver read-only", () => {
    const { r } = fake([theirs(1)], 1);
    as(accountStatus("listener"), <TuningStrip r={r} />);
    expect(screen.getByText("read-only")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Up one step" })).toBeDisabled();
    expect(screen.getByRole("radio", { name: "WFM" })).toBeDisabled();
  });

  it("keeps own receivers editable", () => {
    const { r } = fake([mine(1)], 1);
    as(accountStatus("listener"), <TuningStrip r={r} />);
    expect(screen.queryByText("read-only")).toBeNull();
    expect(screen.getByRole("button", { name: "Up one step" })).toBeEnabled();
  });

  it("listeners cannot bookmark (operators only), with the reason as tooltip", () => {
    const { r } = fake([mine(1)], 1);
    as(accountStatus("listener"), <TuningStrip r={r} />);
    expect(screen.getByRole("button", { name: "Bookmark this frequency" })).toBeDisabled();
  });
});

describe("inspector tabs", () => {
  it("guests lose the bookmarks, and decoders unless the policy shows them", () => {
    const base = { canReadBookmarks: true, canSeeDecoders: true };
    expect(visibleTabs(base)).toEqual([
      "receiver",
      "bookmarks",
      "channels",
      "decoders",
      "display",
      "source",
    ]);
    expect(visibleTabs({ canReadBookmarks: false, canSeeDecoders: false })).toEqual([
      "receiver",
      "channels",
      "display",
      "source",
    ]);
    expect(visibleTabs({ canReadBookmarks: false, canSeeDecoders: true })).toContain("decoders");
  });

  it("the phone tab bar hides what the role cannot see", () => {
    const inspector = {
      open: false,
      tab: "receiver",
      docked: false,
      show: vi.fn(),
      close: vi.fn(),
    } as unknown as InspectorState;
    window.matchMedia = ((q: string) => ({
      matches: false,
      media: q,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    })) as unknown as typeof window.matchMedia;
    as(guestStatus(), <MobileTabBar inspector={inspector} />);
    expect(screen.queryByRole("button", { name: "Bookmarks" })).toBeNull();
    expect(screen.getByRole("button", { name: "Source" })).toBeInTheDocument();
  });
});
