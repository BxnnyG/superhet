import { describe, expect, it } from "vitest";
import type { Rule } from "./api";
import { draftProblem, draftToBody, emptyDraft, ruleToDraft } from "./ruleDraft";

const base = { ...emptyDraft(), name: "Fire", targetIds: [1] };

describe("rule draft", () => {
  it("turns conditions into API documents", () => {
    const body = draftToBody({
      ...base,
      source: " org.superhet.decoder.pocsag ",
      conditions: [
        { field: "data.address", op: "equals", value: "1234", caseSensitive: false },
        { field: "value", op: "contains_any", value: "fire, brand ,", caseSensitive: false },
        { field: "data.address", op: "equals_any", value: "1, 2", caseSensitive: false },
        { field: "value", op: "regex", value: "fire|flood", caseSensitive: true },
        { field: "freq_hz", op: "gte", value: "153000000", caseSensitive: false },
        { field: "data.text", op: "exists", value: "", caseSensitive: false },
      ],
    });
    expect(body.source).toBe("org.superhet.decoder.pocsag");
    expect(body.kind).toBeNull();
    expect(body.conditions).toEqual([
      { field: "data.address", op: "equals", value: 1234 },
      { field: "value", op: "contains", value: ["fire", "brand"] },
      { field: "data.address", op: "equals", value: [1, 2] },
      { field: "value", op: "regex", value: "fire|flood", case_sensitive: true },
      { field: "freq_hz", op: "gte", value: 153000000 },
      { field: "data.text", op: "exists" },
    ]);
  });

  it("sends quiet hours only when switched on", () => {
    expect(draftToBody(base).quiet).toBeNull();
    const on = draftToBody({
      ...base,
      quietOn: true,
      quietStart: "23:00",
      quietEnd: "05:30",
      quietTz: "Europe/Berlin",
    });
    expect(on.quiet).toEqual({ start: "23:00", end: "05:30", tz: "Europe/Berlin" });
  });

  it("round-trips a stored rule, showing lists as the any-of operators", () => {
    const rule: Rule = {
      id: 1,
      name: "Fire",
      enabled: false,
      source: null,
      kind: "pocsag.message",
      conditions: [
        { field: "data.address", op: "equals", value: [1, 2] },
        { field: "value", op: "regex", value: "x", case_sensitive: true },
      ],
      cooldownSecs: 60,
      maxPerHour: 5,
      quiet: { start: "22:00", end: "06:00", tz: "UTC" },
      titleTemplate: "t",
      bodyTemplate: "b",
      targetIds: [3],
    };
    const draft = ruleToDraft(rule);
    expect(draft.conditions[0]).toMatchObject({ op: "equals_any", value: "1, 2" });
    const body = draftToBody(draft);
    expect(body).toMatchObject({
      enabled: false,
      cooldown_secs: 60,
      max_per_hour: 5,
      target_ids: [3],
      kind: "pocsag.message",
    });
    expect(body.conditions).toEqual(rule.conditions);
  });

  it("names the first problem", () => {
    expect(draftProblem({ ...base, name: " " })).toMatch(/name/);
    expect(draftProblem({ ...base, targetIds: [] })).toMatch(/target/);
    expect(
      draftProblem({
        ...base,
        conditions: [{ field: "", op: "equals", value: "x", caseSensitive: false }],
      }),
    ).toMatch(/Condition 1 needs a field/);
    expect(
      draftProblem({
        ...base,
        conditions: [{ field: "a", op: "gt", value: "abc", caseSensitive: false }],
      }),
    ).toMatch(/number/);
    expect(draftProblem(base)).toBeNull();
  });
});
