import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceInfo, DeviceListApi } from "../devices/api";
import { toasts } from "../ui";
import { parseShareLink, shareLinkBody, type AdminApi, type ShareLink } from "./api";
import { ShareLinksSection, inputOf } from "./ShareLinksSection";

beforeEach(() => toasts.clear());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

const device: DeviceInfo = {
  id: "dev0",
  label: "Roof",
  driver: "rtlsdr",
  tuner: "R820T",
  online: true,
  centerHz: 100e6,
  sampleRate: 2_048_000,
  canControl: true,
};

const link = (patch: Partial<ShareLink> = {}): ShareLink => ({
  id: 1,
  tokenHint: "wXyZ",
  label: "Front page",
  title: "Hilltop station",
  description: "",
  location: "Hilltop",
  device: "dev0",
  tuning: "follow",
  freqHz: null,
  mode: null,
  tuneMinHz: null,
  tuneMaxHz: null,
  features: ["waterfall", "audio"],
  maxListeners: 5,
  embedOrigins: [],
  expiresAtMs: null,
  revokedAtMs: null,
  createdAtMs: 0,
  createdBy: "root",
  status: "active",
  listeners: 2,
  ...patch,
});

function mount(links: ShareLink[] = [link()]) {
  const api = {
    getShareLinks: vi.fn().mockResolvedValue(links),
    createShareLink: vi.fn().mockResolvedValue({
      link: link({ id: 2, embedOrigins: ["https://blog.example.org"] }),
      token: "SECRET-TOKEN",
    }),
    updateShareLink: vi.fn(),
    revokeShareLink: vi.fn().mockResolvedValue(link({ status: "revoked" })),
    rotateShareLink: vi.fn(),
    deleteShareLink: vi.fn(),
  };
  const deviceApi: DeviceListApi = { list: vi.fn().mockResolvedValue([device]) };
  render(<ShareLinksSection api={api as unknown as AdminApi} deviceApi={deviceApi} />);
  return api;
}

describe("share links section", () => {
  it("lists links with status, listeners and what they allow, but never a token", async () => {
    mount([
      link(),
      link({ id: 3, label: "Old", status: "revoked", embedOrigins: ["https://x.org"] }),
    ]);
    const list = await screen.findByRole("list", { name: "Share links" });
    const first = within(list).getByTestId("link-1");
    expect(first).toHaveTextContent("Follows the operator");
    expect(first).toHaveTextContent("Waterfall, Audio");
    expect(first).toHaveTextContent("2 of 5 listeners");
    expect(first).toHaveTextContent("not embeddable");
    expect(within(list).getByTestId("link-3")).toHaveTextContent("embeddable on https://x.org");
    expect(within(list).getByText("revoked")).toBeInTheDocument();
    expect(screen.getAllByText(/…wXyZ/)).toHaveLength(2);
  });

  it("creates a link and shows the link and embed code exactly once with copy buttons", async () => {
    const api = mount([]);
    await screen.findByText("No share links yet.");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    fireEvent.click(screen.getByRole("button", { name: "New link" }));
    fireEvent.change(screen.getByLabelText("Name (only you see it)"), {
      target: { value: "Blog" },
    });
    fireEvent.change(screen.getByLabelText("Websites allowed to embed the player"), {
      target: { value: "https://blog.example.org" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create link" }));
    await flush();
    expect(api.createShareLink).toHaveBeenCalledTimes(1);
    expect(api.createShareLink.mock.calls[0]?.[0]).toMatchObject({
      label: "Blog",
      device: "dev0",
      tuning: "follow",
      features: ["waterfall", "audio"],
      embedOrigins: ["https://blog.example.org"],
      expiresAtMs: null,
    });
    const url = `${window.location.origin}/r/SECRET-TOKEN`;
    expect((await screen.findByLabelText("Link for visitors")) as HTMLInputElement).toHaveValue(
      url,
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await flush();
    expect(writeText).toHaveBeenLastCalledWith(url);
    fireEvent.click(screen.getByRole("button", { name: "Copy embed code" }));
    await flush();
    const code = String(writeText.mock.calls.at(-1)?.[0]);
    expect(code).toContain(`<iframe src="${window.location.origin}/embed/SECRET-TOKEN"`);
    expect(code).toContain('allow="autoplay"');
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByText(/SECRET-TOKEN/)).toBeNull();
  });

  it("asks before revoking and calls the API", async () => {
    const api = mount();
    await screen.findByTestId("link-1");
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Revoke" }));
    await flush();
    expect(api.revokeShareLink).toHaveBeenCalledWith(1);
  });

  it("validates the form before sending", async () => {
    const api = mount([]);
    await screen.findByText("No share links yet.");
    fireEvent.click(screen.getByRole("button", { name: "New link" }));
    fireEvent.click(screen.getByRole("button", { name: "Create link" }));
    expect(await screen.findByText("Give the link a name.")).toBeInTheDocument();
    expect(api.createShareLink).not.toHaveBeenCalled();
  });
});

describe("share link wire format", () => {
  const draft = {
    label: "L",
    title: "",
    description: "",
    location: "",
    device: "dev0",
    tuning: "fixed" as const,
    freq: "145.5",
    mode: "nfm",
    min: "144",
    max: "146",
    features: ["audio" as const],
    maxListeners: "3",
    origins: "",
    expiry: "never",
  };

  it("builds a fixed link with a tuning range and refuses inconsistent ranges", () => {
    const input = inputOf(draft, null);
    expect(typeof input).toBe("object");
    if (typeof input === "string") return;
    expect(input).toMatchObject({ freqHz: 145_500_000, tuneMinHz: 144e6, tuneMaxHz: 146e6 });
    expect(shareLinkBody(input)).toMatchObject({
      tuning: "fixed",
      freq_hz: 145_500_000,
      mode: "nfm",
      tune_min_hz: 144e6,
      tune_max_hz: 146e6,
    });
    expect(inputOf({ ...draft, freq: "150" }, null)).toMatch(/inside the tuning range/);
    expect(inputOf({ ...draft, min: "147" }, null)).toMatch(/Tuning range/);
    expect(inputOf({ ...draft, features: [] }, null)).toMatch(/at least one feature/);
    expect(inputOf({ ...draft, maxListeners: "0" }, null)).toMatch(/Listeners/);
  });

  it("follow links send no frequency fields; parsing tolerates odd replies", () => {
    const input = inputOf({ ...draft, tuning: "follow" }, null);
    if (typeof input === "string") throw new Error(input);
    expect(Object.keys(shareLinkBody(input))).not.toContain("freq_hz");
    const parsed = parseShareLink({ id: 4, status: "weird", features: ["audio", "root"] });
    expect(parsed).toMatchObject({ status: "active", features: ["audio"], tuning: "follow" });
  });
});
