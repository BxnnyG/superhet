// Regions: the user's country (default from the browser locale) decides which channel plans are
// "yours". A plan names ISO 3166-1 alpha-2 codes and/or groups; a country belongs to the groups
// listed below. Plans outside the region are never hidden, only sorted below.

// prettier-ignore
const EU = [
  "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT", "LV",
  "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE",
];

/** CEPT members that matter here: the EU plus the usual non-EU European countries. */
// prettier-ignore
const CEPT_EXTRA = [
  "GB", "CH", "NO", "IS", "LI", "AL", "AD", "AM", "AZ", "BA", "GE", "MD", "MC", "ME", "MK", "SM",
  "RS", "TR", "UA", "VA",
];

// prettier-ignore
const R1_EXTRA = [
  "RU", "BY", "KZ", "ZA", "EG", "MA", "DZ", "TN", "NG", "KE", "GH", "SA", "AE", "IL", "QA", "KW",
  "JO", "LB", "OM", "BH", "IQ", "IR",
];

// prettier-ignore
const R2 = [
  "US", "CA", "MX", "BR", "AR", "CL", "CO", "PE", "VE", "UY", "PY", "BO", "EC", "CR", "PA", "GT",
  "DO", "CU", "JM",
];

// prettier-ignore
const R3 = [
  "AU", "NZ", "JP", "KR", "CN", "IN", "TH", "ID", "MY", "SG", "PH", "VN", "PK", "BD", "TW", "HK",
  "LK", "NP",
];

const CEPT = [...EU, ...CEPT_EXTRA];

/** Group name -> member country codes. */
export const REGION_GROUPS: Readonly<Record<string, readonly string[]>> = {
  EU,
  CEPT,
  "ITU-R1": [...CEPT, ...R1_EXTRA],
  "ITU-R2": R2,
  "ITU-R3": R3,
};

/** Group codes a plan may name besides countries. `WORLD` matches everybody. */
export const GROUP_LABEL: Readonly<Record<string, string>> = {
  WORLD: "Worldwide",
  EU: "EU",
  CEPT: "CEPT countries",
  "ITU-R1": "ITU Region 1",
  "ITU-R2": "ITU Region 2",
  "ITU-R3": "ITU Region 3",
};

/** Countries offered in the region picker. */
export const COUNTRY_CODES: readonly string[] = [
  ...new Set([...CEPT, ...R1_EXTRA, ...R2, ...R3]),
].sort();

/** Everything a plan may name that covers `region`: the code itself, its groups and WORLD. */
export function regionTokens(region: string | null): ReadonlySet<string> {
  const tokens = new Set<string>(["WORLD"]);
  if (region === null) return tokens;
  const code = region.toUpperCase();
  tokens.add(code);
  for (const [group, members] of Object.entries(REGION_GROUPS)) {
    if (members.includes(code)) tokens.add(group);
  }
  return tokens;
}

/** Whether a plan that names `planRegions` applies in `region` (null = unknown: nothing does). */
export function regionMatches(planRegions: readonly string[], region: string | null): boolean {
  if (planRegions.includes("WORLD")) return true;
  const tokens = regionTokens(region);
  return planRegions.some((r) => tokens.has(r));
}

/** "de-DE" -> "DE", "en" -> null. */
export function regionFromLocale(locale: string | undefined): string | null {
  if (!locale) return null;
  try {
    // A bare language ("en") says nothing about the country, so there is no guess.
    const region = new Intl.Locale(locale).region;
    return region && /^[A-Z]{2}$/.test(region) ? region : null;
  } catch {
    return null;
  }
}

/** The browser's region, from the first language that names one. */
export function browserRegion(
  languages: readonly string[] = typeof navigator === "undefined"
    ? []
    : (navigator.languages ?? [navigator.language]),
): string | null {
  for (const l of languages) {
    const r = regionFromLocale(l);
    if (r !== null) return r;
  }
  return null;
}

/** "Germany" for "DE"; the code itself when the browser has no name for it. */
export function countryName(code: string): string {
  try {
    return new Intl.DisplayNames(["en"], { type: "region" }).of(code) ?? code;
  } catch {
    return code;
  }
}

/** Short text for a plan's regions: "DE", "CEPT countries", "Worldwide". */
export function describeRegions(regions: readonly string[]): string {
  return regions.map((r) => GROUP_LABEL[r] ?? r).join(", ");
}
