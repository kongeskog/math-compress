export type Metrics = {
  tokens: number;
  loc: number;
  decls: number;
  files: number;
  bytes: number;
  build_seconds?: number | null;
};
export type MetricKey = keyof Metrics;
export type GateStatus = "pass" | "fail" | "pending" | "skip";
export type Gate = { key: string; label: string; status: GateStatus; detail?: string };
export type AttemptStatus =
  | "verifying"
  | "awaiting_review"
  | "stale"
  | "conflict"
  /** a gate failed that the author can fix; open only, never in the attempts log */
  | "needs_fix"
  | "merged"
  | "rejected";
export type Attempt = {
  pr: number;
  title: string;
  author: string;
  head_sha: string;
  base_sha?: string;
  status: AttemptStatus;
  created_at: string;
  decided_at?: string | null;
  metrics?: Metrics | null;
  base_metrics?: Metrics | null;
  /** primary metric change, negative = improvement */
  delta?: number | null;
  gates: Gate[];
  idea?: number | null;
  url: string;
  run_url?: string | null;
  merge_sha?: string | null;
  reason?: string | null;
  technique?: string | null;
  scope?: string | null;
  /** open pull requests this change is built on, the first to merge first */
  depends?: number[];
  /** `pr@sha` of those changes as this commit is judged on top of them */
  judged_on?: string[];
  /** `pr@sha` of the changes its metrics were measured on top of; absent for main alone */
  measured_on?: string[];
  /** notes that fail no gate, such as files reserved by a claimed idea */
  warnings?: string[];
  /** set when a leader waived the minimum step for this commit */
  waiver?: { by: string; reason: string } | null;
};
export type Idea = {
  number: number;
  title: string;
  status: "proposed" | "open" | "claimed" | "done" | "closed";
  author: string;
  claimed_by?: string | null;
  lease_expires?: string | null;
  pr?: number | null;
  result_delta?: number | null;
  created_at: string;
  url: string;
  technique?: string | null;
  scope?: string | null;
};
export type Role = "leader" | "reviewer" | "ideator";
export type Member = { login: string; role: Role; since: string; by: string[] };
export type Application = {
  issue: number;
  login: string;
  role: Role;
  opened_at: string;
  status: "open" | "granted" | "declined";
  endorsements: string[];
  needed: number;
  merged: number;
  saved: number;
  url: string;
};
export type RolesState = { bootstrap: boolean; members: Member[]; applications: Application[] };
export type RoleRules = {
  bootstrap: boolean;
  leaders: string[];
  reviewer_min_merged: number;
  ideator_min_merged: number;
  leader_min_merged: number;
  endorsements: number;
  leader_endorsements: number;
};
export type MetricDef = { key: MetricKey; label: string; unit: string; description: string };
export type State = {
  project: {
    slug: string;
    title: string;
    description: string;
    repo: string;
    dir: string;
    upstream: string;
    primary: string;
    metrics: MetricDef[];
  };
  updated_at: string;
  baseline: { sha: string; at: string; metrics: Metrics };
  current: { sha: string; at: string; metrics: Metrics };
  counts: { merged: number; rejected: number; open: number; contributors: number; ideas_open: number };
  leaderboard: { login: string; merged: number; attempts: number; saved: number }[];
  ideas: Idea[];
  open: Attempt[];
  top_files: { path: string; tokens: number; loc: number }[];
};
export type Config = { repo: string; dataUrl: string; harnessRepo: string };
export type Rules = {
  gates: { key: string; label: string; description: string }[];
  metrics: MetricDef[];
  frozen: string[];
  deny: { name: string; mode: string; description: string }[];
  settings: Record<string, string | number>;
  protect?: { name: string; description: string }[];
  techniques?: { key: string; description: string }[];
  title_format?: string;
  roles?: RoleRules;
  commands?: { command: string; who: string; where: string; does: string }[];
};
export type PromptRole = "invite";

/** Raw pipeline timings written by `hc ops`; the operations page derives everything from them. */
export type OpsVerdict = { at: string; approve: boolean };
export type OpsPr = {
  pr: number;
  title: string;
  status: "open" | "merged" | "closed";
  created_at: string;
  closed_at?: string | null;
  delta?: number | null;
  idea?: number | null;
  verdicts: OpsVerdict[];
};
export type OpsRun = {
  id: number;
  /** 0 is a build of main */
  pr: number;
  kind: "own" | "line" | "main";
  created_at: string;
  started_at: string;
  ended_at?: string | null;
  outcome: string;
};
export type OpsIdea = {
  number: number;
  title: string;
  created_at: string;
  closed_at?: string | null;
  claimed_at?: string | null;
  pr_at?: string | null;
};
export type Ops = { window_start: string; prs: OpsPr[]; runs: OpsRun[]; ideas: OpsIdea[] };
