import { createMemo, onMount, Show } from "solid-js";
import { HillclimbChart } from "~/components/HillclimbChart";
import { store } from "~/lib/data";
import { num, pct } from "~/lib/format";
import type { MetricKey } from "~/lib/types";

/** The 1200x630 card that link previews show. Rendered to og.png at deploy time. */
export default function Og() {
  onMount(() => document.documentElement.classList.add("dark"));
  const primary = createMemo(() => (store.state()?.project.primary ?? "tokens") as MetricKey);
  return (
    <div
      id="og-card"
      class="relative flex flex-col overflow-hidden bg-background text-foreground"
      style={{ width: "1200px", height: "630px", padding: "56px 64px 0" }}
    >
      <Show when={store.state()}>
        {(st) => {
          const base = () => st().baseline.metrics[primary()] ?? 0;
          const cur = () => st().current.metrics[primary()] ?? 0;
          return (
            <>
              <div class="flex items-center gap-3 text-[22px] text-muted-foreground">
                <svg width="34" height="34" viewBox="0 0 32 32" aria-hidden="true">
                  <rect width="32" height="32" rx="7" fill="var(--foreground)" />
                  <path
                    d="M6 9h6v6h6v6h8"
                    fill="none"
                    stroke="var(--background)"
                    stroke-width="2.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
                <span class="font-medium text-foreground">Hillclimb</span>
                <span class="text-faint">/</span>
                <span class="font-mono">{st().project.slug}</span>
              </div>

              <h1 class="mt-7 text-[68px] leading-[1.02] font-semibold tracking-[-0.035em]">
                Shortening the {st().project.title} proof
              </h1>
              <p class="mt-4 max-w-[900px] text-[26px] leading-snug text-muted-foreground">
                An open hillclimb. Every merge is machine-checked: same theorems, fewer tokens.
              </p>

              <div class="mt-8 flex items-baseline gap-5">
                <span class="font-mono text-[52px] font-medium tracking-tight">{num(cur())}</span>
                <span class="text-[24px] text-muted-foreground">tokens</span>
                <span class="font-mono text-[24px] text-good">
                  −{num(Math.max(0, base() - cur()))} ({pct(base() ? ((base() - cur()) / base()) * 100 : 0)})
                </span>
                <span class="text-[24px] text-muted-foreground">
                  · {num(st().counts.merged)} merged
                </span>
              </div>

              <div class="absolute inset-x-0 bottom-0" style={{ height: "210px" }}>
                <HillclimbChart
                  compact
                  height={210}
                  baseline={st().baseline.metrics}
                  baselineAt={st().baseline.at}
                  decided={store.decided()}
                  open={[]}
                  metric={primary()}
                  primary={primary()}
                />
              </div>
            </>
          );
        }}
      </Show>
    </div>
  );
}
