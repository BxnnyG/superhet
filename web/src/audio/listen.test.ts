import { beforeEach, expect, it } from "vitest";
import { listenKey, loadSilent, saveSilent } from "./listen";

beforeEach(() => window.localStorage.clear());

it("round-trips the silent set per device", () => {
  saveSilent("dev0", new Set([3, 1]));
  expect(window.localStorage.getItem(listenKey("dev0"))).toBe('{"silent":[1,3]}');
  expect([...loadSilent("dev0")]).toEqual([1, 3]);
  expect(loadSilent("dev1").size).toBe(0);
});

it("removes the entry when everything listens again", () => {
  saveSilent("dev0", new Set([2]));
  saveSilent("dev0", new Set());
  expect(window.localStorage.getItem(listenKey("dev0"))).toBeNull();
});

it("ignores corrupt data", () => {
  window.localStorage.setItem(listenKey("dev0"), "{nope");
  expect(loadSilent("dev0").size).toBe(0);
  window.localStorage.setItem(listenKey("dev0"), '{"silent":[0,"a",2.5,4]}');
  expect([...loadSilent("dev0")]).toEqual([4]);
});
