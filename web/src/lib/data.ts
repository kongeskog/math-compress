import { createMemo, createRoot, createSignal } from "solid-js";
import type { Attempt, Config, PromptRole, Rules, State } from "./types";

const POLL_MS = 60_000;

function bust(url: string) {
  return `${url}${url.includes("?") ? "&" : "?"}t=${Math.floor(Date.now() / 1000)}`;
}

async function getJson<T>(url: string): Promise<T> {
  const r = await fetch(bust(url), { cache: "no-store" });
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return (await r.json()) as T;
}

async function getText(url: string): Promise<string> {
  const r = await fetch(bust(url), { cache: "no-store" });
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return await r.text();
}

function parseJsonl<T>(text: string): T[] {
  const out: T[] = [];
  for (const line of text.split("\n")) {
    const l = line.trim();
    if (!l) continue;
    try {
      out.push(JSON.parse(l) as T);
    } catch {
      // A partially written trailing line must not break the page.
    }
  }
  return out;
}

type GhPull = {
  number: number;
  title: string;
  html_url: string;
  created_at: string;
  draft?: boolean;
  user: { login: string } | null;
  head: { sha: string };
  base: { sha: string };
};

export const store = createRoot(() => {
  const [config, setConfig] = createSignal<Config>();
  const [state, setState] = createSignal<State>();
  const [decided, setDecided] = createSignal<Attempt[]>([]);
  const [livePulls, setLivePulls] = createSignal<GhPull[]>([]);
  const [rules, setRules] = createSignal<Rules>();
  const [prompts, setPrompts] = createSignal<Partial<Record<PromptRole, string>>>({});
  const [fetchedAt, setFetchedAt] = createSignal<number>();
  const [error, setError] = createSignal<string>();
  const [now, setNow] = createSignal(Date.now());

  let pullsEtag: string | undefined;

  const isRemote = () => /^https?:\/\//.test(config()?.dataUrl ?? "");

  async function refreshData() {
    const c = config();
    if (!c) return;
    const base = c.dataUrl.replace(/\/$/, "");
    try {
      const [s, a] = await Promise.all([
        getJson<State>(`${base}/state.json`),
        getText(`${base}/attempts.jsonl`).catch(() => ""),
      ]);
      setState(s);
      setDecided(
        parseJsonl<Attempt>(a).sort(
          (x, y) =>
            new Date(x.decided_at ?? x.created_at).getTime() -
            new Date(y.decided_at ?? y.created_at).getTime(),
        ),
      );
      setFetchedAt(Date.now());
      setError(undefined);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function refreshPulls() {
    const c = config();
    if (!c || !isRemote()) return;
    try {
      const headers: Record<string, string> = { Accept: "application/vnd.github+json" };
      if (pullsEtag) headers["If-None-Match"] = pullsEtag;
      const r = await fetch(`https://api.github.com/repos/${c.repo}/pulls?state=open&per_page=50`, {
        headers,
      });
      if (r.status === 304 || !r.ok) return;
      pullsEtag = r.headers.get("etag") ?? undefined;
      setLivePulls((await r.json()) as GhPull[]);
    } catch {
      // The overlay is optional; rate limits and offline are expected.
    }
  }

  async function init() {
    try {
      setConfig(await getJson<Config>("./config.json"));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return;
    }
    void refreshData();
    void refreshPulls();
    getJson<Rules>("./rules.json").then(setRules, () => {});
    for (const role of ["contributor", "reviewer", "ideator"] as const) {
      getText(`./prompts/${role}.md`).then(
        (t) => setPrompts((p) => ({ ...p, [role]: t })),
        () => {},
      );
    }
    setInterval(() => {
      if (document.visibilityState === "hidden") return;
      void refreshData();
      void refreshPulls();
    }, POLL_MS);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState !== "visible") return;
      if (Date.now() - (fetchedAt() ?? 0) > POLL_MS) {
        void refreshData();
        void refreshPulls();
      }
    });
    setInterval(() => setNow(Date.now()), 1000);
  }

  /** Open attempts: those in state.json plus open PRs GitHub knows about that the gatekeeper has not recorded yet. */
  const open = createMemo<Attempt[]>(() => {
    const s = state();
    const known = new Set<number>([...(s?.open ?? []), ...decided()].map((a) => a.pr));
    const extra: Attempt[] = livePulls()
      .filter((p) => !known.has(p.number) && !p.draft)
      .map((p) => ({
        pr: p.number,
        title: p.title,
        author: p.user?.login ?? "unknown",
        head_sha: p.head.sha,
        base_sha: p.base.sha,
        status: "verifying",
        created_at: p.created_at,
        gates: [],
        url: p.html_url,
      }));
    return [...(s?.open ?? []), ...extra].sort(
      (a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
    );
  });

  /** Everything, newest first. */
  const all = createMemo<Attempt[]>(() => [...open()].reverse().concat([...decided()].reverse()));

  const find = (pr: number) => all().find((a) => a.pr === pr);

  const repoUrl = () => (config() ? `https://github.com/${config()!.repo}` : "#");

  return {
    config,
    state,
    decided,
    open,
    all,
    find,
    rules,
    prompts,
    fetchedAt,
    error,
    now,
    repoUrl,
    init,
    refresh: refreshData,
  };
});
