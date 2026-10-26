import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ApiError } from "../spectrum/api";
import { toasts } from "../ui";
import { accessCode, accessNotice, reportAccessError } from "./accessErrors";

beforeEach(() => toasts.clear());
afterEach(() => toasts.clear());

// The device routes put the refusal in `code`; the access table answers `{"error":"forbidden"}`.
const device = (code: string) => new ApiError("refused", 403, code);

describe("403 refusals", () => {
  it.each([
    ["not_owner", "Not your receiver"],
    ["vfo_limit", "Receiver limit reached"],
    ["mode_not_allowed", "Mode not allowed"],
    ["frequency_not_allowed", "Frequency not allowed"],
    ["locked", "Tuning is locked"],
  ])("%s has its own wording", (code, title) => {
    expect(accessNotice(device(code))?.title).toBe(title);
  });

  it("plain forbidden and password_change_required come from the error text", () => {
    expect(accessCode(new ApiError("forbidden", 403))).toBe("forbidden");
    expect(accessNotice(new ApiError("forbidden", 403))?.title).toBe("Not allowed");
    expect(accessNotice(new ApiError("password_change_required", 403))?.title).toBe(
      "Change your password first",
    );
  });

  it("an unknown 403 code falls back to the generic text", () => {
    expect(accessNotice(device("something_new"))?.title).toBe("Not allowed");
  });

  it("ignores everything that is not a 403", () => {
    expect(accessNotice(new ApiError("tuning is locked", 409, "locked"))).toBeNull();
    expect(accessNotice(new ApiError("boom", 500))).toBeNull();
    expect(accessNotice(new Error("x"))).toBeNull();
  });

  it("reports one toast per refusal kind", () => {
    expect(reportAccessError(device("vfo_limit"))).toBe(true);
    expect(reportAccessError(device("vfo_limit"))).toBe(true);
    expect(toasts.getSnapshot()).toHaveLength(1);
    expect(toasts.getSnapshot()[0]).toMatchObject({
      tone: "warn",
      title: "Receiver limit reached",
    });
    expect(reportAccessError(new Error("x"))).toBe(false);
  });
});
