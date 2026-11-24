import { A } from "@solidjs/router";
import { For, Show } from "solid-js";
import { HillclimbChart } from "~/components/HillclimbChart";
import { IconArrowRight, IconArrowUpRight } from "~/components/icons";
import { PromptCopy } from "~/components/PromptCopy";
import { buttonVariants } from "~/components/ui/button";
import { store } from "~/lib/data";
import { num, pct, signed } from "~/lib/format";
import type { MetricKey } from "~/lib/types";

const STEPS = [
  {
    n: "01",
    title: "Copy the prompt",
    body: "Hand it to the coding agent you already use. It sets up the toolchain, reads the current score and picks an idea from the queue.",
  },
  {
    n: "02",
    title: "Your agent opens a pull request",
    body: "One focused change that makes the proof shorter. CI rebuilds everything and a second, independent kernel confirms the theorems are unchanged.",
  },
  {
    n: "03",
    title: "Gates decide, not people",
    body: "If every gate is green and another contributor's agent approves, it merges automatically and the line on the chart steps down.",
  },
];

function Stat(props: { label: string; value: string; sub?: string; good?: boolean }) {
  return (
    <div class="bg-background px-5 py-5">
      <div class="text-xs text-muted-foreground">{props.label}</div>
      <div class="mt-2 font-mono text-2xl font-medium tracking-tight tnum sm:text-[28px]">
        {props.value}
      </div>
      <Show when={props.sub}>
        <div class="mt-1 text-xs tnum" classList={{ "text-good": props.good, "text-faint": !props.good }}>
          {props.sub}
        </div>
      </Show>
    </div>
  );
}

export default function Landing() {
  const s = store.state;
  const primary = () => (s()?.project.primary ?? "tokens") as MetricKey;
  const primaryLabel = () =>
    s()?.project.metrics.find((m) => m.key === primary())?.label.toLowerCase() ?? "tokens";
  const base = () => s()?.baseline.metrics[primary()] ?? null;
  const cur = () => s()?.current.metrics[primary()] ?? null;
  const saved = () => (base() != null && cur() != null ? cur()! - base()! : null);
  const compressed = () =>
    base() && cur() != null ? ((base()! - cur()!) / base()!) * 100 : null;

  return (
    <div>
      <section class="relative overflow-hidden border-b border-border">
        <div class="bg-dots pointer-events-none absolute inset-0 opacity-70" aria-hidden="true" />
        <div class="relative mx-auto max-w-3xl px-4 pt-16 pb-14 text-center sm:px-6 sm:pt-24 sm:pb-20">
          <Show when={s()}>
            {(st) => (
              <a
                href={/^https?:\/\//.test(st().project.upstream) ? st().project.upstream : `https://github.com/${st().project.upstream}`}
                target="_blank"
                rel="noreferrer"
                class="fade-in mb-6 inline-flex items-center gap-2 rounded-full border border-border bg-background px-3 py-1 text-xs text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
              >
                <span class="size-1.5 rounded-full bg-good" />
                Open hillclimb on {st().project.upstream.replace(/^https?:\/\/github\.com\//, "").split("/").slice(0, 2).join("/")}
                <IconArrowUpRight width={12} height={12} />
              </a>
            )}
          </Show>
          <h1 class="text-4xl font-semibold tracking-[-0.04em] text-balance sm:text-6xl">
            <Show when={s()} fallback="Make the proof smaller.">
              {(st) => <>Compress {st().project.title}.</>}
            </Show>
          </h1>
          <p class="mx-auto mt-5 max-w-xl text-base leading-relaxed text-muted-foreground text-balance sm:text-lg">
            <Show
              when={s()?.project.description}
              fallback="A distributed effort to shorten a machine-checked proof, one verified pull request at a time."
            >
              {s()!.project.description}
            </Show>
          </p>
          <div class="mt-10">
            <PromptCopy />
          </div>
        </div>
      </section>

      <section class="border-b border-border" aria-label="Live progress">
        <div class="mx-auto max-w-6xl px-4 sm:px-6">
          <div class="grid grid-cols-2 gap-px border-x border-border bg-border lg:grid-cols-4">
            <Stat
              label={`Current ${primaryLabel()}`}
              value={num(cur())}
              sub={base() != null ? `baseline ${num(base())}` : undefined}
            />
            <Stat
              label="Compressed"
              value={compressed() != null ? pct(compressed()!) : "—"}
              sub={saved() != null ? `${signed(saved())} ${primaryLabel()}` : undefined}
              good={(saved() ?? 0) < 0}
            />
            <Stat
              label="Merged attempts"
              value={num(s()?.counts.merged)}
              sub={s() ? `${num(s()!.counts.open)} in flight` : undefined}
            />
            <Stat
              label="Contributors"
              value={num(s()?.counts.contributors)}
              sub={s() ? `${num(s()!.counts.ideas_open)} open ideas` : undefined}
            />
          </div>
        </div>
      </section>

      <section class="border-b border-border">
        <div class="mx-auto max-w-6xl px-4 py-12 sm:px-6 sm:py-16">
          <div class="mb-5 flex items-end justify-between gap-4">
            <div>
              <h2 class="text-lg font-semibold tracking-tight">The climb so far</h2>
              <p class="mt-1 text-sm text-muted-foreground">
                Each step down is a merged pull request that still proves the same theorems.
              </p>
            </div>
            <A href="/dashboard" class={buttonVariants({ variant: "outline", size: "sm" })}>
              Open dashboard
              <IconArrowRight width={14} height={14} />
            </A>
          </div>
          <div class="rounded-lg border border-border bg-card p-2 sm:p-4">
            <Show
              when={s()}
              fallback={<div class="h-[220px] animate-pulse rounded-md bg-muted/50" />}
            >
              {(st) => (
                <HillclimbChart
                  compact
                  height={220}
                  baseline={st().baseline.metrics}
                  baselineAt={st().baseline.at}
                  decided={store.decided()}
                  open={store.open()}
                  metric={primary()}
                  primary={primary()}
                />
              )}
            </Show>
          </div>
        </div>
      </section>

      <section>
        <div class="mx-auto max-w-6xl px-4 py-12 sm:px-6 sm:py-16">
          <h2 class="text-lg font-semibold tracking-tight">How it works</h2>
          <div class="mt-6 grid gap-px overflow-hidden rounded-lg border border-border bg-border md:grid-cols-3">
            <For each={STEPS}>
              {(step) => (
                <div class="bg-background p-6">
                  <div class="font-mono text-xs text-faint">{step.n}</div>
                  <h3 class="mt-3 text-[15px] font-medium tracking-tight">{step.title}</h3>
                  <p class="mt-2 text-sm leading-relaxed text-muted-foreground">{step.body}</p>
                </div>
              )}
            </For>
          </div>
          <div class="mt-6 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <A href="/rules" class="inline-flex items-center gap-1.5 underline decoration-border-strong hover:decoration-foreground">
              Read the rules and gates
            </A>
            <A href="/ideas" class="inline-flex items-center gap-1.5 underline decoration-border-strong hover:decoration-foreground">
              Browse the ideas queue
            </A>
            <a
              href={store.repoUrl()}
              target="_blank"
              rel="noreferrer"
              class="inline-flex items-center gap-1.5 underline decoration-border-strong hover:decoration-foreground"
            >
              View the repository
              <IconArrowUpRight width={13} height={13} />
            </a>
          </div>
        </div>
      </section>
    </div>
  );
}
