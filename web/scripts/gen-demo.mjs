// Generates deterministic demo data into public/demo so the app can be developed without a live repo.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const out = join(dirname(fileURLToPath(import.meta.url)), "..", "public", "demo");
mkdirSync(out, { recursive: true });

let seed = 20261123;
const rnd = () => {
  seed = (seed * 1664525 + 1013904223) >>> 0;
  return seed / 2 ** 32;
};
const pick = (xs) => xs[Math.floor(rnd() * xs.length)];
const int = (lo, hi) => Math.floor(lo + rnd() * (hi - lo + 1));
const hex = (n = 40) => Array.from({ length: n }, () => "0123456789abcdef"[int(0, 15)]).join("");

const REPO = "kongeskog/formal-math";
const authors = ["mira-k", "tobiasz", "lean-otter", "hszabo", "qfwd", "ana-rios", "pnkfelix2", "jdvries", "nyx-0", "okabe-r"];
const files = [
  "Paper/Tree.lean", "Paper/Groups.lean", "Paper/Fan.lean", "Paper/Alternative.lean", "Paper/Pieces.lean",
  "Nc/Dl/SizeDeal.lean", "Paper/Identity.lean", "Paper/Flat.lean", "Paper/Gauges.lean", "NS1/Lists.lean",
  "Nc/G/PlayThm.lean", "Nc/Tow/Load0.lean", "NS1/Comp.lean", "Nc/Hc/Segments.lean", "Nc/Grp/GroupProg.lean",
];
const scopeOf = (f) => f.replace(/\.lean$/, "");
const techniques = [
  ["dead-code", (f) => `remove unused lemmas from ${f}`],
  ["shorten", (f) => `replace manual rewrite chains with simp in ${f}`],
  ["merge", (f) => `merge duplicate helper lemmas in ${f}`],
  ["library", (f) => `use the Mathlib lemma instead of the local copy in ${f}`],
  ["shorten", (f) => `collapse case splits with omega in ${f}`],
  ["restructure", (f) => `inline single-use definitions in ${f}`],
  ["generalize", (f) => `one generalized induction statement in ${f}`],
  ["shorten", (f) => `drop redundant hypotheses in ${f}`],
  ["restructure", (f) => `factor the shared bound out of ${f}`],
  ["shorten", (f) => `shorter term-mode proofs in ${f}`],
];
/** A standard title plus its parsed parts. */
const titled = (file) => {
  const [technique, text] = pick(techniques);
  const scope = scopeOf(file);
  return { title: `${technique}(${scope}): ${text(file.split("/").pop().replace(/\.lean$/, ""))}`, technique, scope };
};
const rejectReasons = [
  ["improves", "No improvement: tokens did not drop by the minimum of 25"],
  ["build", "lake build failed"],
  ["frozen", "Frozen file changed: Statement.lean"],
  ["deny", "Banned construct: native_decide"],
  ["improves", "Guard regressed: build time 1.6× baseline"],
  ["comparator", "Comparator rejected the solution"],
  ["deny", "Line longer than 100 characters"],
  ["scope", "Files outside the project changed: README.md"],
];
// The arena of the demo requires no approval, so there is no review gate.
const GATES = [
  ["format", "Standard title and description"],
  ["frozen", "Frozen files unchanged"],
  ["scope", "Changes stay inside the project"],
  ["deny", "No banned constructs"],
  ["added_lines", "Added lines are clean"],
  ["deleted_names", "No deleted name still mentioned"],
  ["conduct", "Automation within the stated limits"],
  ["comments", "Added text within limits"],
  ["build", "Compiles without sorry"],
  ["comparator", "Same theorems, same axioms"],
  ["improves", "Score improves, guards hold"],
  ["fresh", "Verified on current main"],
];
const gates = (failKey, pendingFrom) => {
  let failed = false;
  let pending = false;
  return GATES.map(([key, label]) => {
    if (key === pendingFrom) pending = true;
    if (failed) return { key, label, status: "skip" };
    if (pending) return { key, label, status: "pending" };
    if (key === failKey) {
      failed = true;
      return { key, label, status: "fail", detail: rejectReasons.find((r) => r[0] === key)?.[1] };
    }
    return { key, label, status: "pass" };
  });
};

const HOUR = 3_600_000;
const now = Date.now();
const start = now - 5 * 24 * HOUR;
const baseline = { tokens: 928_161, loc: 58_797, decls: 4_610, files: 321, bytes: 3_421_123, build_seconds: 1_486 };
const baselineSha = hex();

let cur = { ...baseline };
let curSha = baselineSha;
let pr = 1;
const attempts = [];
const N = 60;
const stats = new Map();
const bump = (login, merged, saved) => {
  const s = stats.get(login) ?? { login, merged: 0, attempts: 0, saved: 0 };
  s.attempts += 1;
  if (merged) {
    s.merged += 1;
    s.saved += saved;
  }
  stats.set(login, s);
};

const ideas = [];
const ideaTitles = [
  ["dead-code", "*", "delete declarations unreachable from Solution"],
  ["restructure", "*", "narrow imports where it shortens the open lines"],
  ["merge", "Nc", "unify the three copies of Computes behind one definition"],
  ["restructure", "Solution", "prove directly against Statement, dropping toNc and ofNc"],
  ["generalize", "Paper/Tree", "generalize the subtree bound lemmas"],
  ["merge", "Nc/Tow", "Load0 and Load2 share most of their case analysis"],
  ["shorten", "Paper/Rate", "use gcongr for the monotonicity blocks"],
  ["library", "NS1/Lists", "List lemmas that already exist in Mathlib"],
  ["restructure", "*", "split the 16 maxHeartbeats proofs into smaller lemmas"],
  ["shorten", "Paper/Groups", "replace explicit coset arithmetic with group"],
  ["shorten", "Paper/Fan", "a single strong induction"],
  ["shorten", "Nc/G/Move", "turn repeated have chains into one calc"],
  ["restructure", "Paper/Identity", "the entries macro can be a simp set"],
  ["merge", "Nc/Grp", "GroupProg and GrpTime prove the same bound twice"],
];
ideaTitles.forEach(([technique, scope, text], k) => {
  ideas.push({
    number: 100 + k,
    title: `Idea: ${technique}(${scope}): ${text}`,
    technique,
    scope,
    status: "open",
    author: pick(authors),
    claimed_by: null,
    lease_expires: null,
    pr: null,
    result_delta: null,
    created_at: new Date(start + rnd() * 3.5 * 24 * HOUR).toISOString(),
    url: `https://github.com/${REPO}/issues/${100 + k}`,
  });
});

for (let k = 0; k < N; k++) {
  const decided = start + ((k + 1) / (N + 1)) * (now - start - 2 * HOUR) + (rnd() - 0.5) * HOUR;
  const created = decided - int(25, 190) * 60_000;
  const author = pick(authors);
  const file = pick(files);
  const merged = rnd() < 0.46;
  const number = pr++;
  const base = { ...cur };
  const baseSha = curSha;
  const a = {
    pr: number,
    ...titled(file),
    author,
    head_sha: hex(),
    base_sha: baseSha,
    status: merged ? "merged" : "rejected",
    created_at: new Date(created).toISOString(),
    decided_at: new Date(decided).toISOString(),
    metrics: null,
    base_metrics: base,
    delta: null,
    gates: [],
    idea: null,
    url: `https://github.com/${REPO}/pull/${number}`,
    run_url: `https://github.com/${REPO}/actions/runs/${9_000_000 + number * 137}`,
    merge_sha: null,
    reason: null,
  };
  if (merged) {
    // Mostly small wins, occasionally a big one.
    const big = rnd() < 0.18;
    const saved = big ? int(2200, 6000) : int(50, 1400);
    const m = {
      tokens: base.tokens - saved,
      loc: base.loc - Math.round(saved / int(11, 17)),
      decls: base.decls - (big ? int(4, 30) : int(0, 5)),
      files: base.files - (big && rnd() < 0.3 ? 1 : 0),
      bytes: base.bytes - saved * int(3, 5),
      build_seconds: Math.max(900, base.build_seconds + int(-25, 14)),
    };
    a.metrics = m;
    a.delta = -saved;
    a.gates = gates();
    a.merge_sha = hex();
    cur = m;
    curSha = a.merge_sha;
    bump(author, true, saved);
    if (rnd() < 0.22 && ideas.filter((i) => i.status === "done").length < 5) {
      const idea = ideas.find((i) => i.status === "open");
      if (idea) {
        idea.status = "done";
        idea.pr = number;
        idea.result_delta = -saved;
        idea.claimed_by = author;
        a.idea = idea.number;
      }
    }
  } else {
    const [key, reason] = pick(rejectReasons);
    a.reason = reason;
    a.gates = gates(key);
    const staticFail = key === "frozen" || key === "deny" || key === "scope";
    if (!staticFail && key !== "build") {
      const d = int(-20, 600);
      a.metrics = {
        tokens: base.tokens + d,
        loc: base.loc + Math.round(d / 14),
        decls: base.decls,
        files: base.files,
        bytes: base.bytes + d * 4,
        build_seconds: Math.round(base.build_seconds * (reason.includes("1.6") ? 1.6 : 1)),
      };
      a.delta = d;
    } else if (staticFail) {
      a.metrics = { ...base, tokens: base.tokens - int(30, 900), build_seconds: null };
      a.delta = a.metrics.tokens - base.tokens;
    }
    bump(author, false, 0);
  }
  attempts.push(a);
}

const open = [];
const openSpecs = [
  { status: "needs_fix", failKey: "added_lines", saved: 742, reason: "Added lines are clean: 1 added line(s) longer than 100 characters: vp-vnp/Paper/Tree.lean:412 (104)" },
  { status: "verifying", pendingFrom: "build", saved: 318 },
  { status: "stale", pendingFrom: "fresh", saved: 1290, reason: "Main moved; branch updated and verification re-running" },
  { status: "verifying", pendingFrom: "frozen", saved: null },
];
openSpecs.forEach((spec, k) => {
  const number = pr++;
  const author = pick(authors);
  const hasMetrics = spec.saved != null;
  const a = {
    pr: number,
    ...titled(pick(files)),
    author,
    head_sha: hex(),
    base_sha: curSha,
    status: spec.status,
    created_at: new Date(now - (openSpecs.length - k) * 26 * 60_000).toISOString(),
    decided_at: null,
    metrics: hasMetrics
      ? { ...cur, tokens: cur.tokens - spec.saved, loc: cur.loc - Math.round(spec.saved / 14), bytes: cur.bytes - spec.saved * 4, build_seconds: null }
      : null,
    base_metrics: hasMetrics ? { ...cur } : null,
    delta: hasMetrics ? -spec.saved : null,
    gates: hasMetrics ? gates(spec.failKey, spec.pendingFrom) : [],
    idea: null,
    url: `https://github.com/${REPO}/pull/${number}`,
    run_url: `https://github.com/${REPO}/actions/runs/${9_000_000 + number * 137}`,
    merge_sha: null,
    reason: spec.reason ?? null,
  };
  bump(author, false, 0);
  open.push(a);
});

// Two claimed ideas, one closed.
const stillOpen = ideas.filter((i) => i.status === "open");
if (stillOpen[0]) {
  Object.assign(stillOpen[0], { status: "claimed", claimed_by: open[0].author, lease_expires: new Date(now + 95 * 60_000).toISOString(), pr: open[0].pr });
  open[0].idea = stillOpen[0].number;
}
if (stillOpen[1]) Object.assign(stillOpen[1], { status: "claimed", claimed_by: pick(authors), lease_expires: new Date(now + 22 * 60_000).toISOString() });
if (stillOpen[2]) Object.assign(stillOpen[2], { status: "closed" });
// Two ideas from newcomers, waiting for triage.
if (stillOpen[3]) Object.assign(stillOpen[3], { status: "proposed", author: "first-timer" });
if (stillOpen[4]) Object.assign(stillOpen[4], { status: "proposed", author: "curious-cat" });

const metricDefs = [
  { key: "tokens", label: "Tokens", unit: "tokens", description: "Lean source tokens after stripping comments and whitespace. Lower is better." },
  { key: "loc", label: "Lines", unit: "lines", description: "Non-blank, non-comment lines of Lean." },
  { key: "decls", label: "Declarations", unit: "decls", description: "Theorems, lemmas, definitions, instances and structures." },
  { key: "files", label: "Files", unit: "files", description: "Lean source files counted." },
  { key: "bytes", label: "Bytes", unit: "bytes", description: "Size of the counted source on disk." },
  { key: "build_seconds", label: "Build time", unit: "seconds", description: "Wall-clock time of the full build in CI. Noisy; used as a loose guard." },
];

const leaderboard = [...stats.values()].sort((a, b) => b.saved - a.saved || b.merged - a.merged);
const merged = attempts.filter((a) => a.status === "merged");
const topTokens = [21480, 15102, 13377, 12240, 11896, 10411, 9987, 8530, 8122, 7719, 7410, 7003, 6950, 6611, 6204];

const state = {
  project: {
    slug: "vp-vnp",
    title: "VP ≠ VNP",
    description:
      "A distributed effort to shorten the Lean 4 formalization of VP ≠ VNP, one machine-checked pull request at a time. Every merge still proves the same five theorems from the same three axioms.",
    repo: REPO,
    dir: "vp-vnp",
    upstream: "anthropics/formal-math",
    primary: "tokens",
    metrics: metricDefs,
  },
  updated_at: new Date(now).toISOString(),
  baseline: { sha: baselineSha, at: new Date(start).toISOString(), metrics: baseline },
  current: { sha: curSha, at: merged.at(-1)?.decided_at ?? new Date(start).toISOString(), metrics: cur },
  counts: {
    merged: merged.length,
    rejected: attempts.length - merged.length,
    open: open.length,
    contributors: stats.size,
    ideas_open: ideas.filter((i) => i.status === "open").length,
  },
  leaderboard,
  ideas,
  open,
  top_files: files.map((path, k) => ({ path, tokens: topTokens[k], loc: Math.round(topTokens[k] / 13) })),
};

const iso = (hoursAgo) => new Date(now - hoursAgo * HOUR).toISOString();
const roles = {
  bootstrap: true,
  members: [
    { login: "kongeskog", role: "leader", since: iso(120), by: [] },
    { login: "mira-k", role: "leader", since: iso(30), by: ["kongeskog"] },
    { login: "ana-rios", role: "ideator", since: iso(70), by: ["kongeskog"] },
    { login: "okabe-r", role: "ideator", since: iso(41), by: ["kongeskog", "mira-k"] },
    { login: "lean-otter", role: "ideator", since: iso(55), by: ["kongeskog"] },
  ],
  applications: [
    {
      issue: 131,
      login: "qfwd",
      role: "ideator",
      opened_at: iso(3),
      status: "open",
      endorsements: [],
      needed: 1,
      merged: 2,
      saved: 2432,
      url: `https://github.com/${REPO}/issues/131`,
    },
    {
      issue: 118,
      login: "tobiasz",
      role: "ideator",
      opened_at: iso(40),
      status: "granted",
      endorsements: ["kongeskog"],
      needed: 1,
      merged: 1,
      saved: 1330,
      url: `https://github.com/${REPO}/issues/118`,
    },
  ],
};
writeFileSync(join(out, "roles.json"), JSON.stringify(roles, null, 2) + "\n");
writeFileSync(join(out, "state.json"), JSON.stringify(state, null, 2) + "\n");
writeFileSync(join(out, "attempts.jsonl"), attempts.map((a) => JSON.stringify(a)).join("\n") + "\n");
console.log(
  `demo: ${attempts.length} decided (${merged.length} merged), ${open.length} open, ${ideas.length} ideas, tokens ${baseline.tokens} -> ${cur.tokens}`,
);
