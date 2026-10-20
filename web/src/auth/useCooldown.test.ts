import { expect, it } from "vitest";
import { formatCountdown } from "./useCooldown";

it("formats seconds below a minute and m:ss from a minute up", () => {
  expect(formatCountdown(0)).toBe("0s");
  expect(formatCountdown(59)).toBe("59s");
  expect(formatCountdown(60)).toBe("1:00");
  expect(formatCountdown(65)).toBe("1:05");
  expect(formatCountdown(885)).toBe("14:45");
  expect(formatCountdown(3600)).toBe("60:00");
});
