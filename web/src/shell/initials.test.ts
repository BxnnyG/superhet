import { expect, it } from "vitest";
import { initials } from "./AccountMenu";

it.each([
  ["root", "R"],
  ["alice.smith", "AS"],
  ["bob_the-builder", "BT"],
  ["  ", "?"],
  ["émile", "É"],
])("initials of %s", (name, want) => {
  expect(initials(name)).toBe(want);
});
