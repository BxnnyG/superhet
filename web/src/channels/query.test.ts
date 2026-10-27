import { describe, expect, it } from "vitest";
import { builtinPlans } from "./catalog";
import { resolveChannelQuery } from "./query";

const plans = builtinPlans();
const ask = (q: string, region: string | null = "DE") =>
  resolveChannelQuery(plans, q, region).map((h) => `${h.plan.id}/${h.channel.number}`);

describe("channel words in the palette", () => {
  it("resolves plan word and number, with or without a space", () => {
    expect(ask("pmr 3")).toEqual(["pmr446/3"]);
    expect(ask("pmr3")).toEqual(["pmr446/3"]);
    expect(ask("PMR 16")).toEqual(["pmr446/16"]);
    expect(ask("pmr446 3")).toEqual(["pmr446/3"]);
    expect(ask("pmr4463")).toEqual(["pmr446/3"]);
    expect(ask("freenet 2")).toEqual(["freenet-de/2"]);
    expect(ask("freenet2")).toEqual(["freenet-de/2"]);
    expect(ask("lpd 69")).toEqual(["lpd433/69"]);
  });

  it("resolves DAB blocks and the marine channels", () => {
    expect(ask("dab 5c")).toEqual(["dab-band3/5C"]);
    expect(ask("dab5c")).toEqual(["dab-band3/5C"]);
    expect(ask("dab 13f")).toEqual(["dab-band3/13F"]);
    expect(ask("marine 16")).toEqual(["marine-vhf/16"]);
    expect(ask("ch16")).toEqual(["marine-vhf/16"]);
    expect(ask("ch 16")).toEqual(["marine-vhf/16"]);
  });

  it("ignores leading zeros and case in channel numbers", () => {
    expect(ask("marine 06")).toEqual(["marine-vhf/06"]);
    expect(ask("marine 6")).toEqual(["marine-vhf/06"]);
    expect(ask("ch6")).toEqual(["marine-vhf/06"]);
    expect(ask("dab 5C")).toEqual(["dab-band3/5C"]);
    expect(ask("coast 87b")).toEqual(["marine-vhf-coast/87B"]);
  });

  it("finds a number in every plan that answers to the word, region first", () => {
    expect(ask("cb 19")).toEqual(["cb-eu/19"]);
    expect(ask("cb 45")).toEqual(["cb-de-ext/45"]);
    expect(ask("cb 45", "FR")).toEqual(["cb-de-ext/45"]);
  });

  it("answers nothing for words that are not channels", () => {
    expect(ask("")).toEqual([]);
    expect(ask("pmr")).toEqual([]);
    expect(ask("pmr 17")).toEqual([]);
    expect(ask("pmr 0")).toEqual([]);
    expect(ask("dab 14a")).toEqual([]);
    expect(ask("nothing 3")).toEqual([]);
    expect(ask("446.00625")).toEqual([]);
    expect(ask("145.5 nfm")).toEqual([]);
  });

  it("puts plans of the user's region first when several answer", () => {
    const both = [
      ...plans,
      ...plans.filter((p) => p.id === "cb-eu").map((p) => ({ ...p, id: "cb-us", regions: ["US"] })),
    ];
    const hits = (region: string) =>
      resolveChannelQuery(both, "cb 19", region).map((h) => h.plan.id);
    expect(hits("DE")).toEqual(["cb-eu", "cb-us"]);
    expect(hits("US")).toEqual(["cb-us", "cb-eu"]);
  });
});
