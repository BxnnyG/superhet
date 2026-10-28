import { describe, expect, it, vi } from "vitest";
import {
  blockingInstance,
  claimBadge,
  claimHint,
  claimOwner,
  conflictText,
  shortPluginName,
  startReplacing,
} from "./claim";
import { adsb, instance, pocsag } from "./testing";

const DAB = "org.superhet.decoder.dab";
const ADSB = "org.superhet.decoder.adsb";
const claim = { kind: "claim", id: "i-dab", sinceMs: 1 };

describe("claimOwner", () => {
  it("is null while nobody holds a claim", () => {
    expect(claimOwner([{ kind: "user", id: "user", sinceMs: 1 }], [], [])).toBeNull();
  });

  it("names the plugin from the instance list (operators)", () => {
    const owner = claimOwner([claim], [instance({ id: "i-dab", plugin_id: DAB })], []);
    expect(owner).toEqual({ instanceId: "i-dab", pluginId: DAB, name: "DAB" });
    expect(claimBadge(owner!)).toBe("DAB has the device");
  });

  it("falls back to what the DAB view follows (everybody else)", () => {
    const owner = claimOwner([claim], [], [], [{ instanceId: "i-dab", pluginId: DAB }]);
    expect(owner?.name).toBe("DAB");
  });

  it("stays neutral when the viewer cannot tell", () => {
    const owner = claimOwner([claim], [], []);
    expect(owner).toMatchObject({ pluginId: null, name: "A plugin" });
  });
});

describe("claimHint", () => {
  it("says where the centre is and that the VFO resumes", () => {
    const owner = { instanceId: "i-dab", pluginId: DAB, name: "DAB" };
    const hint = claimHint(owner, 185_360_000, 1_089_812_500);
    expect(hint).toContain("DAB is using the whole device at 185.360 MHz");
    expect(hint).toContain("keeps 1089.8125 MHz and resumes when DAB stops");
  });
});

describe("blockingInstance", () => {
  const running = instance({
    id: "i-adsb",
    plugin_id: ADSB,
    exclusive: true,
    device: "dev0",
    state: "running",
  });

  it("finds the other exclusive plugin on the same device", () => {
    expect(blockingInstance([running], "dev0", DAB)?.id).toBe("i-adsb");
    expect(shortPluginName([adsb], ADSB)).toBe("ADS-B");
    expect(conflictText(running, [], "DAB")).toBe(
      "ADS-B is using the device. Stop ADS-B to start DAB.",
    );
  });

  it("ignores itself, other devices, stopped or failed instances and shared decoders", () => {
    expect(blockingInstance([running], "dev0", ADSB)).toBeNull();
    expect(blockingInstance([running], "dev1", DAB)).toBeNull();
    expect(blockingInstance([{ ...running, state: "stopped" }], "dev0", DAB)).toBeNull();
    expect(blockingInstance([{ ...running, state: "error" }], "dev0", DAB)).toBeNull();
    expect(blockingInstance([instance({ plugin_id: pocsag.id })], "dev0", DAB)).toBeNull();
  });
});

describe("startReplacing", () => {
  const req = { pluginId: DAB, device: "dev0", freqHz: 185_360_000 };
  const blocker = instance({ id: "i-adsb", plugin_id: ADSB, exclusive: true });

  it("stops the blocker first, then starts", async () => {
    const order: string[] = [];
    const plugins = {
      stopInstance: vi.fn(async (id: string) => void order.push(`stop ${id}`)),
      startInstance: vi.fn(async () => {
        order.push("start");
        return instance({ id: "i-new" });
      }),
    };
    const started = await startReplacing(plugins, blocker, req);
    expect(started.id).toBe("i-new");
    expect(order).toEqual(["stop i-adsb", "start"]);
  });

  it("just starts when nothing is in the way, and does not start if the stop failed", async () => {
    const plugins = {
      stopInstance: vi.fn(async () => {
        throw new Error("forbidden");
      }),
      startInstance: vi.fn(async () => instance({ id: "i-new" })),
    };
    await startReplacing(plugins, null, req);
    expect(plugins.stopInstance).not.toHaveBeenCalled();
    await expect(startReplacing(plugins, blocker, req)).rejects.toThrow("forbidden");
    expect(plugins.startInstance).toHaveBeenCalledTimes(1);
  });
});
