import { describe, expect, it } from "vitest";
import { adminSectionOf, inviteTokenOf } from "./router";

describe("routes", () => {
  it("reads the token of an invitation link", () => {
    expect(inviteTokenOf("/invite/abc_DEF-123")).toBe("abc_DEF-123");
    expect(inviteTokenOf("/invite/abc/")).toBe("abc");
    expect(inviteTokenOf("/invite/")).toBeNull();
    expect(inviteTokenOf("/invite/a/b")).toBeNull();
    expect(inviteTokenOf("/admin")).toBeNull();
    expect(inviteTokenOf("/")).toBeNull();
  });

  it("reads the admin console section", () => {
    expect(adminSectionOf("/admin")).toBe("");
    expect(adminSectionOf("/admin/")).toBe("");
    expect(adminSectionOf("/admin/audit-settings")).toBe("audit-settings");
    expect(adminSectionOf("/administrator")).toBeNull();
    expect(adminSectionOf("/")).toBeNull();
    expect(adminSectionOf("/admin/a/b")).toBeNull();
  });
});
