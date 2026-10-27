// The built-in channel plans: every `plans/*.json` is validated once at startup. A broken file
// is a build-time mistake, so it throws (the unit tests load the same list).

import { parsePlan, type Plan } from "./schema";

const files = import.meta.glob<unknown>("./plans/*.json", { eager: true, import: "default" });

function load(): Plan[] {
  return Object.entries(files)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, raw]) => {
      try {
        return parsePlan(raw, { builtin: true });
      } catch (e) {
        throw new Error(`channel plan ${path}: ${e instanceof Error ? e.message : String(e)}`, {
          cause: e,
        });
      }
    });
}

let cached: readonly Plan[] | null = null;

/** The shipped plans, sorted by file name. */
export function builtinPlans(): readonly Plan[] {
  cached ??= load();
  return cached;
}
