import { A, useNavigate } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import { AttemptRow } from "~/components/AttemptRow";
import { ChartLegend, HillclimbChart } from "~/components/HillclimbChart";
import { Empty, Page, WithState } from "~/components/Page";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { Segmented } from "~/components/ui/segmented";
import { store } from "~/lib/data";
import { metricValue, num, pct, signed } from "~/lib/format";
import type { MetricKey, State } from "~/lib/types";

function StatCard(props: { label: string; value: string; sub?: string; good?: boolean; wide?: boolean }) {
  return (
    <div class="bg-card px-5 py-4" classList={{ "col-span-2 md:col-span-1": props.wide }}>
      <div class="text-xs text-muted-foreground">{props.label}</div>
      <div class="mt-1.5 font-mono text-xl font-medium tracking-tight tnum">{props.value}</div>
      <div class="mt-0.5 min-h-4 text-xs tnum" classList={{ "text-good": props.good, "text-faint": !props.good }}>
        {props.sub}
      </div>
    </div>
  );
}

function Body(props: { s: State }) {
  const navigate = useNavigate();
  const primary = () => props.s.project.primary as MetricKey;
  const [metric, setMetric] = createSignal<MetricKey>(primary());
  const [xMode, setXMode] = createSignal<"order" | "time">("order");

  const metricOptions = createMemo(() =>
    props.s.project.metrics.map((m) => ({ value: m.key, label: m.label })),
  );
  const def = () => props.s.project.metrics.find((m) => m.key === metric());

  const base = () => props.s.baseline.metrics[primary()] ?? 0;
  const cur = () => props.s.current.metrics[primary()] ?? 0;
  const decidedCount = () => props.s.counts.merged + props.s.counts.rejected;
  const feed = createMemo(() => store.all().slice(0, 12));
  const maxFile = () => Math.max(1, ...props.s.top_files.map((f) => f.tokens));

  const selBase = () => props.s.baseline.metrics[metric()];
  const selCur = () => props.s.current.metrics[metric()];

  return (
    <>
      <div class="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border md:grid-cols-5">
        <StatCard
          wide
          label={`Current ${def()?.key === primary() ? def()!.label.toLowerCase() : "score"}`}
          value={num(cur())}
          sub={`${signed(cur() - base())} vs baseline`}
          good={cur() < base()}
        />
        <StatCard
          label="Compressed"
          value={base() ? pct(((base() - cur()) / base()) * 100) : "—"}
          sub={`from ${num(base())}`}
        />
        <StatCard
          label="Merged"
          value={num(props.s.counts.merged)}
          sub={
            decidedCount()
              ? `${pct((props.s.counts.merged / decidedCount()) * 100, 0)} of ${num(decidedCount())} decided`
              : "no attempts yet"
          }
        />
        <StatCard
          label="In flight"
          value={num(store.open().length)}
          sub={`${num(props.s.counts.contributors)} contributors`}
        />
        <StatCard
          label="Open ideas"
          value={num(props.s.counts.ideas_open)}
          sub={`${num(props.s.ideas.filter((i) => i.status === "claimed").length)} claimed`}
        />
      </div>

      <Card class="mt-6">
        <CardHeader class="flex-col items-start gap-3 sm:flex-row sm:items-center">
          <div class="min-w-0">
            <CardTitle>{def()?.label ?? metric()}</CardTitle>
            <div class="mt-0.5 text-xs text-muted-foreground tnum">
              {metricValue(metric(), selCur())}
              <Show when={selBase() != null && selCur() != null && metric() !== "build_seconds"}>
                <span class="text-faint"> · {signed(selCur()! - selBase()!)} since baseline</span>
              </Show>
            </div>
          </div>
          <div class="flex max-w-full flex-wrap items-center gap-2">
            <Segmented
              label="Metric"
              value={metric()}
              options={metricOptions()}
              onChange={setMetric}
            />
            <Segmented
              label="Horizontal axis"
              value={xMode()}
              options={[
                { value: "order", label: "Attempt" },
                { value: "time", label: "Time" },
              ]}
              onChange={setXMode}
            />
          </div>
        </CardHeader>
        <div class="px-2 pt-3 sm:px-3">
          <HillclimbChart
            baseline={props.s.baseline.metrics}
            baselineAt={props.s.baseline.at}
            decided={store.decided()}
            open={store.open()}
            metric={metric()}
            metricLabel={def()?.label}
            primary={primary()}
            xMode={xMode()}
            onSelect={(pr) => navigate(`/attempts/${pr}`)}
          />
        </div>
        <div class="flex flex-col gap-2 border-t border-border px-5 py-3 sm:flex-row sm:items-center sm:justify-between">
          <ChartLegend />
          <Show when={def()?.description}>
            <p class="max-w-md text-xs text-faint sm:text-right">{def()!.description}</p>
          </Show>
        </div>
      </Card>

      <div class="mt-6 grid gap-6 lg:grid-cols-5">
        <Card class="overflow-hidden lg:col-span-3">
          <CardHeader>
            <CardTitle>Activity</CardTitle>
            <A href="/attempts" class="text-xs text-muted-foreground hover:text-foreground">
              All attempts →
            </A>
          </CardHeader>
          <Show when={feed().length} fallback={<Empty>No attempts yet. Be the first.</Empty>}>
            <div class="divide-y divide-border">
              <For each={feed()}>{(a) => <AttemptRow a={a} />}</For>
            </div>
          </Show>
        </Card>

        <div class="flex flex-col gap-6 lg:col-span-2">
          <Card class="overflow-hidden">
            <CardHeader>
              <CardTitle>Leaderboard</CardTitle>
              <span class="text-xs text-faint">saved</span>
            </CardHeader>
            <Show when={props.s.leaderboard.length} fallback={<Empty>Nobody on the board yet.</Empty>}>
              <ol class="divide-y divide-border">
                <For each={props.s.leaderboard.slice(0, 8)}>
                  {(l, i) => (
                    <li class="flex items-center gap-3 px-5 py-2.5 text-sm">
                      <span class="w-5 font-mono text-xs text-faint tnum">{i() + 1}</span>
                      <a
                        href={`https://github.com/${l.login}`}
                        target="_blank"
                        rel="noreferrer"
                        class="min-w-0 flex-1 truncate hover:underline"
                      >
                        {l.login}
                      </a>
                      <span class="text-xs text-muted-foreground tnum">
                        {l.merged}/{l.attempts}
                      </span>
                      <span class="w-20 text-right font-mono text-good tnum">
                        {signed(-Math.abs(l.saved))}
                      </span>
                    </li>
                  )}
                </For>
              </ol>
            </Show>
          </Card>

          <Card class="overflow-hidden">
            <CardHeader>
              <CardTitle>Largest files</CardTitle>
              <span class="text-xs text-faint">tokens</span>
            </CardHeader>
            <Show when={props.s.top_files.length} fallback={<Empty>No file data.</Empty>}>
              <ul class="divide-y divide-border">
                <For each={props.s.top_files.slice(0, 8)}>
                  {(f) => (
                    <li class="relative px-5 py-2.5 text-sm">
                      <div
                        class="absolute inset-y-0 left-0 bg-muted/70"
                        style={{ width: `${(f.tokens / maxFile()) * 100}%` }}
                        aria-hidden="true"
                      />
                      <div class="relative flex items-center gap-3">
                        <a
                          href={`${store.repoUrl()}/blob/main/${f.path}`}
                          target="_blank"
                          rel="noreferrer"
                          class="min-w-0 flex-1 truncate font-mono text-xs hover:underline"
                        >
                          {f.path.startsWith(`${props.s.project.dir}/`) ? f.path.slice(props.s.project.dir.length + 1) : f.path}
                        </a>
                        <span class="font-mono text-xs text-muted-foreground tnum">{num(f.tokens)}</span>
                      </div>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Card>
        </div>
      </div>
    </>
  );
}

export default function Dashboard() {
  return (
    <Page
      title="Dashboard"
      subtitle={
        <Show when={store.state()} fallback="Live progress of the hillclimb.">
          {(s) => (
            <>
              Live progress on <span class="font-mono text-foreground">{s().project.repo}</span>, directory{" "}
              <span class="font-mono text-foreground">{s().project.dir}/</span>.
            </>
          )}
        </Show>
      }
    >
      <WithState>{(s) => <Body s={s()} />}</WithState>
    </Page>
  );
}
