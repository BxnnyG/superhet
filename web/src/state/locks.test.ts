import { expect, it } from "vitest";
import { describeHolders, holdsUserLock, isLocked, lockedBy, onlyUserLock } from "./locks";

const user = { kind: "user", id: "user", sinceMs: 1 };
const rec = { kind: "recording", id: "r1", sinceMs: 2 };

it("words the holders", () => {
  expect(lockedBy([user])).toBe("Locked by you");
  expect(describeHolders([rec, user])).toBe("you and a recording");
  expect(describeHolders([{ kind: "scanner", id: "s", sinceMs: 0 }])).toBe("scanner");
});

it("classifies holders", () => {
  expect(isLocked([])).toBe(false);
  expect(isLocked([user])).toBe(true);
  expect(holdsUserLock([rec])).toBe(false);
  expect(onlyUserLock([user])).toBe(true);
  expect(onlyUserLock([user, rec])).toBe(false);
  expect(onlyUserLock([])).toBe(false);
});
