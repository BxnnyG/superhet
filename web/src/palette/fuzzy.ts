// Fuzzy matching for the command palette: every whitespace-separated term of the query must be a
// subsequence of the text. Contiguous runs, word starts and an early start score higher; an exact
// or prefix match beats a scattered one. Pure.

const WORD_START = /[\s\-_./:,()]/;

/** Score of one term against a text (higher is better), or null when it does not match. */
export function termScore(term: string, text: string): number | null {
  const t = term.toLowerCase();
  const s = text.toLowerCase();
  if (t === "") return 0;
  const at = s.indexOf(t);
  if (at >= 0) {
    // Substring: prefix > word start > inside; shorter texts win.
    const wordStart = at === 0 || WORD_START.test(s.charAt(at - 1));
    return 100 + (at === 0 ? 60 : wordStart ? 40 : 0) + (t.length === s.length ? 40 : 0) - at * 0.5;
  }
  // Subsequence with greedy matching, rewarding adjacency and word starts.
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of t) {
    const i = s.indexOf(ch, from);
    if (i < 0) return null;
    score += 10;
    if (i === prev + 1) score += 8;
    if (i === 0 || WORD_START.test(s.charAt(i - 1))) score += 6;
    score -= Math.min(i - from, 6) * 0.5;
    prev = i;
    from = i + 1;
  }
  return score;
}

/** Score of a query against one text; null unless every term matches. */
export function fuzzyScore(query: string, text: string): number | null {
  const terms = query
    .trim()
    .split(/\s+/)
    .filter((q) => q !== "");
  if (terms.length === 0) return 0;
  let total = 0;
  for (const term of terms) {
    const s = termScore(term, text);
    if (s === null) return null;
    total += s;
  }
  return total - text.length * 0.05;
}

/** Best score over several texts (title, keywords, ...) with the first weighted highest. */
export function bestScore(query: string, texts: readonly string[]): number | null {
  let best: number | null = null;
  texts.forEach((text, i) => {
    const s = fuzzyScore(query, text);
    if (s === null) return;
    const weighted = s - i * 4;
    if (best === null || weighted > best) best = weighted;
  });
  return best;
}

/**
 * Items matching the query, best first. The sort is stable, so equal scores keep their input
 * order. An empty query keeps everything in input order.
 */
export function rank<T>(
  items: readonly T[],
  query: string,
  texts: (item: T) => readonly string[],
): T[] {
  return rankScored(items, query, texts).map((s) => s.item);
}

/** Like rank() but keeps the scores (0 for an empty query). */
export function rankScored<T>(
  items: readonly T[],
  query: string,
  texts: (item: T) => readonly string[],
): { item: T; score: number }[] {
  if (query.trim() === "") return items.map((item) => ({ item, score: 0 }));
  const scored: { item: T; score: number; index: number }[] = [];
  items.forEach((item, index) => {
    const score = bestScore(query, texts(item));
    if (score !== null) scored.push({ item, score, index });
  });
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map(({ item, score }) => ({ item, score }));
}
