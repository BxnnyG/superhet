import { describe, expect, it } from "vitest";
import { auditQuery } from "./api";
import {
  EMPTY_FILTER,
  activeFilters,
  filterProblem,
  parseLocalDateTime,
  toFilter,
} from "./auditFilter";

const NOW = Date.UTC(2026, 9, 26, 12, 0, 0);

describe("audit filter form", () => {
  it("turns the presets into a lower bound relative to now", () => {
    expect(toFilter({ ...EMPTY_FILTER, range: "1h" }, NOW)).toEqual({ fromMs: NOW - 3_600_000 });
    expect(toFilter({ ...EMPTY_FILTER, range: "7d" }, NOW).fromMs).toBe(NOW - 7 * 86_400_000);
    expect(toFilter({ ...EMPTY_FILTER, range: "all" }, NOW)).toEqual({});
  });

  it("uses the custom range and ignores empty fields", () => {
    const f = toFilter({ ...EMPTY_FILTER, range: "custom", from: "2026-10-01T08:00", to: "" }, NOW);
    expect(f.fromMs).toBe(new Date("2026-10-01T08:00").getTime());
    expect(f.toMs).toBeUndefined();
  });

  it("trims text and keeps only set fields", () => {
    expect(
      toFilter(
        {
          ...EMPTY_FILTER,
          range: "all",
          actor: " alice ",
          role: "operator",
          action: "user.*",
          outcome: "denied",
          ip: " 198.51.100.0/24 ",
          actorKind: "guest",
          target: "dev0",
        },
        NOW,
      ),
    ).toEqual({
      actor: "alice",
      role: "operator",
      action: "user.*",
      outcome: "denied",
      ip: "198.51.100.0/24",
      actorKind: "guest",
      target: "dev0",
    });
  });

  it("rejects an inverted custom range", () => {
    expect(
      filterProblem({
        ...EMPTY_FILTER,
        range: "custom",
        from: "2026-10-02T00:00",
        to: "2026-10-01T00:00",
      }),
    ).toMatch(/before/);
    expect(
      filterProblem({ ...EMPTY_FILTER, range: "custom", from: "2026-10-01T00:00", to: "" }),
    ).toBeNull();
    expect(filterProblem(EMPTY_FILTER)).toBeNull();
  });

  it("counts the narrowing fields", () => {
    expect(activeFilters({ ...EMPTY_FILTER, range: "all" })).toBe(0);
    expect(activeFilters({ ...EMPTY_FILTER, actor: "x", ip: "1.2.3.4" })).toBe(3);
  });

  it("parses local date times", () => {
    expect(parseLocalDateTime("")).toBeNull();
    expect(parseLocalDateTime("nonsense")).toBeNull();
    expect(parseLocalDateTime("2026-10-01T08:00")).toBe(new Date("2026-10-01T08:00").getTime());
  });
});

describe("audit query string", () => {
  it("uses the documented parameter names", () => {
    const q = auditQuery(
      {
        fromMs: 5,
        toMs: 9,
        actorKind: "user",
        actor: "alice",
        role: "admin",
        action: "user.*",
        outcome: "ok",
        ip: "198.51.100.0/24",
        target: "t",
      },
      { limit: 100, before_id: 42, format: "csv" },
    );
    const p = new URLSearchParams(q.slice(1));
    expect(Object.fromEntries(p)).toEqual({
      from_ms: "5",
      to_ms: "9",
      actor_kind: "user",
      actor: "alice",
      role: "admin",
      action: "user.*",
      outcome: "ok",
      ip: "198.51.100.0/24",
      target: "t",
      limit: "100",
      before_id: "42",
      format: "csv",
    });
  });

  it("is empty without filters", () => {
    expect(auditQuery({})).toBe("");
  });
});
