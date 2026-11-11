import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { NotificationsApi, Rule, Target } from "../notifications/api";
import { toasts } from "../ui";
import { NotificationsSection } from "./NotificationsSection";

beforeEach(() => toasts.clear());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

const hook: Target = {
  id: 1,
  name: "ntfy phone",
  kind: "webhook",
  config: { url_host: "https://ntfy.example.com" },
  hasSecret: true,
};

const existing: Rule = {
  id: 7,
  name: "Fire pager",
  enabled: true,
  source: "org.superhet.decoder.pocsag",
  kind: "pocsag.message",
  conditions: [{ field: "data.address", op: "equals", value: [1234, 5678] }],
  cooldownSecs: 60,
  maxPerHour: null,
  quiet: { start: "22:00", end: "06:00", tz: "Europe/Berlin" },
  titleTemplate: "",
  bodyTemplate: "",
  targetIds: [1],
};

function fakeApi(rules: Rule[] = [existing]) {
  const api = {
    listTargets: vi.fn().mockResolvedValue([hook]),
    createTarget: vi.fn(),
    updateTarget: vi.fn(),
    deleteTarget: vi.fn().mockResolvedValue(undefined),
    testTarget: vi.fn().mockResolvedValue(undefined),
    listRules: vi.fn().mockResolvedValue(rules),
    createRule: vi.fn().mockResolvedValue(existing),
    updateRule: vi.fn().mockResolvedValue(existing),
    deleteRule: vi.fn().mockResolvedValue(undefined),
    testRule: vi.fn().mockResolvedValue(undefined),
    listDeliveries: vi.fn().mockResolvedValue({
      deliveries: [
        {
          id: 3,
          ruleName: "Fire pager",
          targetName: "ntfy phone",
          eventId: 9,
          status: "failed",
          attempts: 5,
          lastError: "HTTP 503 Service Unavailable",
          title: "Fire pager",
          body: "x",
          createdAtMs: Date.now(),
          finishedAtMs: Date.now(),
        },
      ],
      nextBefore: null,
    }),
  };
  return api satisfies NotificationsApi;
}

async function mount(api = fakeApi()) {
  render(<NotificationsSection api={api} />);
  await screen.findByRole("list", { name: "Rules" }).catch(() => undefined);
  await flush();
  return api;
}

describe("rule list", () => {
  it("shows the rule with its filters and targets", async () => {
    await mount();
    const row = screen.getByTestId("rule-7");
    expect(row).toHaveTextContent("Fire pager");
    expect(row).toHaveTextContent("org.superhet.decoder.pocsag");
    expect(row).toHaveTextContent("quiet 22:00-06:00");
    expect(row).toHaveTextContent("ntfy phone");
  });

  it("queues a test send and says where to look", async () => {
    const api = await mount();
    fireEvent.click(
      within(screen.getByTestId("rule-7")).getByRole("button", { name: "Send test" }),
    );
    await flush();
    expect(api.testRule).toHaveBeenCalledWith(7);
    expect(toasts.getSnapshot().some((t) => t.title === "Test queued")).toBe(true);
  });
});

describe("rule editor", () => {
  it("creates a rule with a condition and a target", async () => {
    const api = await mount(fakeApi([]));
    fireEvent.click(screen.getByRole("button", { name: "New rule" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Callsign watch" },
    });
    fireEvent.change(within(dialog).getByLabelText("Source (plugin id or core)"), {
      target: { value: "org.superhet.decoder.adsb" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add condition" }));
    fireEvent.change(within(dialog).getByLabelText("Field"), {
      target: { value: "data.callsign" },
    });
    fireEvent.change(within(dialog).getByLabelText("Comparison"), {
      target: { value: "equals_any" },
    });
    fireEvent.change(within(dialog).getByLabelText("Value"), { target: { value: "DLH123, BAW9" } });
    // Without a target the editor refuses.
    fireEvent.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(within(dialog).getByText("Pick at least one target.")).toBeInTheDocument();
    expect(api.createRule).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("switch", { name: "ntfy phone (webhook)" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await flush();
    expect(api.createRule).toHaveBeenCalledWith(
      expect.objectContaining({
        name: "Callsign watch",
        source: "org.superhet.decoder.adsb",
        kind: null,
        conditions: [{ field: "data.callsign", op: "equals", value: ["DLH123", "BAW9"] }],
        target_ids: [1],
        quiet: null,
      }),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("loads a stored rule and saves changes to it, including quiet hours", async () => {
    const api = await mount();
    fireEvent.click(screen.getByRole("button", { name: "Edit Fire pager" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByLabelText("Name")).toHaveValue("Fire pager");
    expect(within(dialog).getByLabelText("Value")).toHaveValue("1234, 5678");
    expect(within(dialog).getByLabelText("Time zone")).toHaveValue("Europe/Berlin");
    fireEvent.change(within(dialog).getByLabelText("Value"), { target: { value: "1234" } });
    fireEvent.change(within(dialog).getByLabelText("Comparison"), { target: { value: "equals" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await flush();
    expect(api.updateRule).toHaveBeenCalledWith(
      7,
      expect.objectContaining({
        conditions: [{ field: "data.address", op: "equals", value: 1234 }],
        quiet: { start: "22:00", end: "06:00", tz: "Europe/Berlin" },
        cooldown_secs: 60,
      }),
    );
  });

  it("shows the server's reason when it rejects the rule", async () => {
    const api = await mount(fakeApi([]));
    api.createRule.mockRejectedValueOnce(new Error("condition 1: invalid pattern"));
    fireEvent.click(screen.getByRole("button", { name: "New rule" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "x" } });
    fireEvent.click(within(dialog).getByRole("switch", { name: "ntfy phone (webhook)" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save rule" }));
    await flush();
    expect(within(dialog).getByText("condition 1: invalid pattern")).toBeInTheDocument();
  });
});

describe("targets and log", () => {
  it("never shows a secret or URL, only the host and that a secret is set", async () => {
    await mount();
    fireEvent.click(screen.getByRole("radio", { name: "Targets" }));
    await flush();
    const row = screen.getByTestId("target-1");
    expect(row).toHaveTextContent("https://ntfy.example.com");
    expect(row).toHaveTextContent("secret set");
    fireEvent.click(within(row).getByRole("button", { name: "Edit ntfy phone" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByLabelText(/^URL/)).toHaveValue("");
    expect(within(dialog).getByLabelText(/^Secret/)).toHaveValue("");
  });

  it("keeps the stored secret and URL when saving without typing new ones", async () => {
    const api = await mount();
    api.updateTarget.mockResolvedValue(hook);
    fireEvent.click(screen.getByRole("radio", { name: "Targets" }));
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Edit ntfy phone" }));
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Save target" }),
    );
    await flush();
    const [, body] = api.updateTarget.mock.calls[0] as [number, Record<string, unknown>];
    expect("secret" in body).toBe(false);
    expect("url" in (body.config as object)).toBe(false);
  });

  it("lists deliveries with status and last error", async () => {
    await mount();
    fireEvent.click(screen.getByRole("radio", { name: "Delivery log" }));
    await flush();
    const row = screen.getByTestId("delivery-3");
    expect(row).toHaveTextContent("failed");
    expect(row).toHaveTextContent("HTTP 503 Service Unavailable");
    expect(row).toHaveTextContent("5 attempts");
  });
});
