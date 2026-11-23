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
};
export type Idea = {
  number: number;
  title: string;
  status: "open" | "claimed" | "done" | "closed";
  author: string;
  claimed_by?: string | null;
  lease_expires?: string | null;
  pr?: number | null;
  result_delta?: number | null;
  created_at: string;
  url: string;
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
};
export type PromptRole = "contributor" | "reviewer" | "ideator";
