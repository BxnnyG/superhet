import { afterEach, describe, expect, it } from "vitest";
import {
  describeHolders,
  foreignUserLock,
  holdsUserLock,
  lockedBy,
  onlyUserLock,
  setLockIdentity,
} from "./locks";

const alice = { kind: "user", id: "u:7", name: "alice", sinceMs: 1 };
const bob = { kind: "user", id: "u:8", name: "bob", sinceMs: 2 };
const legacy = { kind: "user", id: "user", sinceMs: 3 };
const rec = { kind: "recording", id: "r1", sinceMs: 4 };

afterEach(() => setLockIdentity(undefined));

describe("lock holders with accounts", () => {
  it("my own lock reads 'you', the others use the account name", () => {
    expect(lockedBy([alice], "u:7")).toBe("Locked by you");
    expect(lockedBy([bob], "u:7")).toBe("Locked by bob");
    expect(describeHolders([bob, alice], "u:7")).toBe("you and bob");
    expect(describeHolders([rec, bob], "u:7")).toBe("a recording and bob");
  });

  it("a nameless user lock (hidden from guests, or from before accounts) is 'another user'", () => {
    expect(lockedBy([legacy], "u:7")).toBe("Locked by another user");
    expect(lockedBy([{ ...bob, name: undefined }], null)).toBe("Locked by another user");
  });

  it("classifies by identity", () => {
    expect(holdsUserLock([alice, bob], "u:7")).toBe(true);
    expect(holdsUserLock([bob], "u:7")).toBe(false);
    expect(onlyUserLock([alice], "u:7")).toBe(true);
    expect(onlyUserLock([alice, bob], "u:7")).toBe(false);
    expect(foreignUserLock([alice, bob], "u:7")).toBe(true);
    expect(foreignUserLock([alice], "u:7")).toBe(false);
    expect(foreignUserLock([rec], "u:7")).toBe(false);
  });

  it("a guest holds nothing", () => {
    expect(holdsUserLock([alice], null)).toBe(false);
    expect(foreignUserLock([alice], null)).toBe(true);
  });

  it("uses the identity set by the auth gate", () => {
    setLockIdentity("u:8");
    expect(lockedBy([bob, alice])).toBe("Locked by you and alice");
    setLockIdentity(undefined);
    // Unknown identity (single-user setups): every user lock is the viewer's own.
    expect(lockedBy([alice])).toBe("Locked by you");
  });
});
