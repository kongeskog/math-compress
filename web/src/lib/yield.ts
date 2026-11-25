/**
 * Where the reduction comes from: merged changes grouped by technique, size, scope and time.
 * Pure functions over attempts; the Yield page only renders what these return.
 */
import type { Attempt } from "./types";

export const OTHER = "other";

export type Change = {
  pr: number;
  title: string;
  url: string;
  technique: string;
  /** top-level folder of the scope, `*` when cross-cutting, `other` when the title does not parse */
  area: string;
  /** tokens removed, positive = saved */
  saved: number;
  at: string;
};

export type Group = { key: string; n: number; saved: number; share: number; median: number; mean: number };
export type SizeClass = { label: string; min: number; max: number; n: number; saved: number; share: number };
export type Period = { label: string; from: number; to: number; n: number; saved: number; median: number; largest: number };
export type Failure = { label: string; n: number };

const TITLE = /^\s*([A-Za-z][\w-]*)\s*\(([^)]*)\)\s*:\s*\S/;

/** `technique(scope): summary`, or undefined when the title does not have that form. */
export function parseTitle(title: string): { technique: string; scope: string } | undefined {
  const m = TITLE.exec(title);
  return m ? { technique: m[1].toLowerCase(), scope: m[2].trim() } : undefined;
}

/** The top-level folder of a scope; several folders, or `*`, are cross-cutting. */
export function topLevel(scope: string): string {
  const tops = new Set(
    scope
      .split(/[,\s]+/)
      .map((s) => s.replace(/^\.?\//, "").split("/")[0])
      .filter(Boolean),
  );
  if (tops.size !== 1 || tops.has("*")) return "*";
  return [...tops][0];
}

export function median(xs: number[]): number {
  if (!xs.length) return 0;
  const s = [...xs].sort((a, b) => a - b);
  const h = s.length >> 1;
  return s.length % 2 ? s[h] : (s[h - 1] + s[h]) / 2;
}

const sum = (xs: number[]) => xs.reduce((s, x) => s + x, 0);

/** Merged attempts as changes, in merge order. `techniques` are the arena's; anything else is `other`. */
export function changes(attempts: Attempt[], techniques: string[]): Change[] {
  const known = new Set(techniques);
  return attempts
    .filter((a) => a.status === "merged")
    .map((a) => {
      const p = parseTitle(a.title);
      const technique = p && (!known.size || known.has(p.technique)) ? p.technique : OTHER;
      return {
        pr: a.pr,
        title: a.title,
        url: a.url,
        technique,
        area: p ? topLevel(p.scope) : OTHER,
        saved: -(a.delta ?? 0),
        at: a.decided_at ?? a.created_at,
      };
    })
    .sort((x, y) => new Date(x.at).getTime() - new Date(y.at).getTime() || x.pr - y.pr);
}

/** Changes grouped by a key, largest saving first. */
export function groupBy(cs: Change[], key: (c: Change) => string): Group[] {
  const total = sum(cs.map((c) => c.saved));
  const by = new Map<string, number[]>();
  for (const c of cs) by.set(key(c), [...(by.get(key(c)) ?? []), c.saved]);
  return [...by]
    .map(([k, xs]) => {
      const s = sum(xs);
      return { key: k, n: xs.length, saved: s, share: total ? s / total : 0, median: median(xs), mean: s / xs.length };
    })
    .sort((a, b) => b.saved - a.saved || b.n - a.n || a.key.localeCompare(b.key));
}

/** Changes largest first, with the running share of the total after each. */
export function concentration(cs: Change[]): { change: Change; cum: number; share: number }[] {
  const total = sum(cs.map((c) => c.saved));
  let cum = 0;
  return [...cs]
    .sort((a, b) => b.saved - a.saved || a.pr - b.pr)
    .map((change) => {
      cum += change.saved;
      return { change, cum, share: total ? cum / total : 0 };
    });
}

export const BANDS: [string, number, number][] = [
  ["Under 100", -Infinity, 100],
  ["100–300", 100, 300],
  ["300–1,000", 300, 1000],
  ["1,000–3,000", 1000, 3000],
  ["Over 3,000", 3000, Infinity],
];

export function sizeClasses(cs: Change[]): SizeClass[] {
  const total = sum(cs.map((c) => c.saved));
  return BANDS.map(([label, min, max]) => {
    const xs = cs.filter((c) => c.saved >= min && c.saved < max).map((c) => c.saved);
    const s = sum(xs);
    return { label, min, max, n: xs.length, saved: s, share: total ? s / total : 0 };
  });
}

/** The merge sequence cut into up to `k` consecutive runs of equal length. */
export function periods(cs: Change[], k = 4): Period[] {
  const n = Math.min(k, cs.length);
  const out: Period[] = [];
  for (let i = 0; i < n; i++) {
    const from = Math.floor((i * cs.length) / n);
    const to = Math.floor(((i + 1) * cs.length) / n);
    const xs = cs.slice(from, to).map((c) => c.saved);
    out.push({
      label: `${from + 1}–${to}`,
      from,
      to,
      n: xs.length,
      saved: sum(xs),
      median: median(xs),
      largest: Math.max(...xs),
    });
  }
  return out;
}

/**
 * The gate that failed, by its label, for attempts that were not merged.
 * An attempt that needs a fix is not among them: it is open, its author is
 * repairing it, and nothing was decided about it.
 */
export function failures(attempts: Attempt[]): Failure[] {
  const by = new Map<string, number>();
  for (const a of attempts) {
    if (a.status === "merged" || a.status === "needs_fix") continue;
    const gate = a.gates.find((g) => g.status === "fail");
    if (!gate && a.status !== "rejected") continue;
    const label = gate?.label ?? a.reason?.split(":")[0]?.trim() ?? "No reason given";
    by.set(label, (by.get(label) ?? 0) + 1);
  }
  return [...by].map(([label, n]) => ({ label, n })).sort((a, b) => b.n - a.n || a.label.localeCompare(b.label));
}

export function yieldOf(attempts: Attempt[], techniques: string[]) {
  const cs = changes(attempts, techniques);
  return {
    changes: cs,
    total: sum(cs.map((c) => c.saved)),
    byTechnique: groupBy(cs, (c) => c.technique),
    byArea: groupBy(cs, (c) => c.area),
    concentration: concentration(cs),
    sizes: sizeClasses(cs),
    periods: periods(cs),
    failures: failures(attempts),
  };
}
