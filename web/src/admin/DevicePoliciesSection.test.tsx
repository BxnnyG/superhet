import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceInfo, DeviceListApi } from "../devices/api";
import { ApiError } from "../spectrum/api";
import { toasts } from "../ui";
import { openPolicy, parseDevicePolicy, type AdminApi, type DevicePolicy, type User } from "./api";
import { DevicePoliciesSection, isOpen, summary } from "./DevicePoliciesSection";

beforeEach(() => toasts.clear());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

const device = (id: string, label: string | null = null, online = true): DeviceInfo => ({
  id,
  label,
  driver: "rtlsdr",
  tuner: "R820T",
  online,
  centerHz: 100e6,
  sampleRate: 2_048_000,
  canControl: true,
});

const user = (id: number, username: string, role: User["role"]): User => ({
  id,
  username,
  role,
  disabled: false,
  mustChangePassword: false,
  createdAtMs: 0,
  lastLoginMs: null,
  sessions: 0,
  email: null,
  oidc: false,
});

const USERS = [user(1, "root", "admin"), user(2, "olga", "operator"), user(3, "lars", "listener")];

function mount(policies: DevicePolicy[] = [], devices: DeviceInfo[] = [device("dev0", "Roof")]) {
  const api = {
    getDevicePolicies: vi.fn().mockResolvedValue(policies),
    listUsers: vi.fn().mockResolvedValue(USERS),
    putDevicePolicy: vi
      .fn()
      .mockImplementation((p: DevicePolicy) => Promise.resolve({ policy: p, endedSessions: 0 })),
    resetDevicePolicy: vi.fn().mockImplementation((d: string) => Promise.resolve(openPolicy(d))),
  };
  const deviceApi: DeviceListApi = { list: vi.fn().mockResolvedValue(devices) };
  render(<DevicePoliciesSection api={api as unknown as AdminApi} deviceApi={deviceApi} />);
  return { api, deviceApi };
}

const card = (id: string) => within(screen.getByTestId(`device-${id}`));

describe("device access section", () => {
  it("lists every device with its policy summary and defaults to open", async () => {
    mount([{ ...openPolicy("rtl1"), controlRole: "operator" }], [device("dev0", "Roof")]);
    await screen.findByRole("list", { name: "Devices" });
    // A policy of a device whose daemon is gone stays editable.
    expect(screen.getByText("Roof")).toBeInTheDocument();
    expect(screen.getByText("Open to everyone")).toBeInTheDocument();
    expect(screen.getByText("rtl1")).toBeInTheDocument();
    expect(screen.getByText("changed by operators and up")).toBeInTheDocument();
    // Nothing to save before something changes.
    expect(card("dev0").getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(card("dev0").queryByRole("button", { name: "Reset to open" })).toBeNull();
  });

  it("saves a stricter policy for one device", async () => {
    const { api } = mount();
    await screen.findByText("Roof");
    fireEvent.change(card("dev0").getByLabelText("Who can see and listen"), {
      target: { value: "operator" },
    });
    fireEvent.change(card("dev0").getByLabelText("Who can change it"), {
      target: { value: "admin" },
    });
    fireEvent.click(card("dev0").getByRole("switch", { name: "Guests can see it" }));
    expect(card("dev0").getByText("Unsaved changes")).toBeInTheDocument();
    fireEvent.click(card("dev0").getByRole("button", { name: "Save changes" }));
    await flush();
    expect(api.putDevicePolicy).toHaveBeenCalledWith({
      device: "dev0",
      viewRole: "operator",
      controlRole: "admin",
      guestsVisible: false,
      allowedUsers: [],
    });
    expect(toasts.getSnapshot().some((t) => t.title === "Access to Roof saved")).toBe(true);
  });

  it("limits a device to chosen accounts, never offering admins", async () => {
    const { api } = mount();
    await screen.findByText("Roof");
    fireEvent.click(card("dev0").getByRole("switch", { name: "Only selected accounts" }));
    const accounts = within(screen.getByRole("list", { name: "Accounts allowed on Roof" }));
    expect(accounts.queryByText("root")).toBeNull();
    // Listing nobody is not a policy: save stays off until an account is picked.
    expect(card("dev0").getByRole("button", { name: "Save changes" })).toBeDisabled();
    fireEvent.click(accounts.getByRole("button", { name: "lars" }));
    fireEvent.click(accounts.getByRole("button", { name: "olga" }));
    fireEvent.click(card("dev0").getByRole("button", { name: "Save changes" }));
    await flush();
    expect(api.putDevicePolicy).toHaveBeenCalledWith(
      expect.objectContaining({ device: "dev0", allowedUsers: [2, 3] }),
    );
  });

  it("tells how many sessions lost the device, resets to open and shows refusals", async () => {
    const policy: DevicePolicy = { ...openPolicy("dev0"), viewRole: "operator" };
    const { api } = mount([policy]);
    await screen.findByText("seen by operators and up");
    expect(card("dev0").getByLabelText("Who can see and listen")).toHaveValue("operator");

    api.putDevicePolicy.mockResolvedValueOnce({
      policy: { ...policy, viewRole: "admin" },
      endedSessions: 2,
    });
    fireEvent.change(card("dev0").getByLabelText("Who can see and listen"), {
      target: { value: "admin" },
    });
    fireEvent.click(card("dev0").getByRole("button", { name: "Save changes" }));
    await flush();
    expect(
      toasts.getSnapshot().some((t) => t.body === "2 sessions that lost access were disconnected."),
    ).toBe(true);

    api.resetDevicePolicy.mockRejectedValueOnce(new ApiError("database error", 503));
    fireEvent.click(card("dev0").getByRole("button", { name: "Reset to open" }));
    await flush();
    expect(await screen.findByRole("alert")).toHaveTextContent(/database error/);
    fireEvent.click(card("dev0").getByRole("button", { name: "Reset to open" }));
    await flush();
    expect(api.resetDevicePolicy).toHaveBeenLastCalledWith("dev0");
  });

  it("still shows the stored policies when the device list is unavailable", async () => {
    const api = {
      getDevicePolicies: vi.fn().mockResolvedValue([openPolicy("rtl1")]),
      listUsers: vi.fn().mockResolvedValue(USERS),
    };
    const deviceApi: DeviceListApi = { list: vi.fn().mockRejectedValue(new ApiError("bus", 502)) };
    render(<DevicePoliciesSection api={api as unknown as AdminApi} deviceApi={deviceApi} />);
    expect(await screen.findByTestId("device-rtl1")).toBeInTheDocument();
  });
});

describe("policy helpers", () => {
  it("summarises who gets what", () => {
    expect(isOpen(openPolicy("a"))).toBe(true);
    expect(summary(openPolicy("a"))).toBe("Open to everyone");
    expect(summary({ ...openPolicy("a"), guestsVisible: false })).toBe("hidden from guests");
    expect(summary({ ...openPolicy("a"), allowedUsers: [2, 3] })).toBe(
      "2 selected accounts and admins only",
    );
    expect(summary({ ...openPolicy("a"), allowedUsers: [2], controlRole: "admin" })).toBe(
      "1 selected account and admins only, changed by admins and up",
    );
  });

  it("reads the server's policy and falls back to open values", () => {
    expect(
      parseDevicePolicy({
        device: "rtl1",
        view_role: "operator",
        control_role: "admin",
        guests_visible: false,
        allowed_users: [4, "x", 9],
      }),
    ).toEqual({
      device: "rtl1",
      viewRole: "operator",
      controlRole: "admin",
      guestsVisible: false,
      allowedUsers: [4, 9],
    });
    expect(parseDevicePolicy({ device: "x", view_role: "root" })).toEqual(openPolicy("x"));
    expect(() => parseDevicePolicy({})).toThrow();
  });
});
