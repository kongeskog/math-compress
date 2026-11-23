import { For, Show } from "solid-js";
import { Empty, Page } from "~/components/Page";
import { Badge } from "~/components/ui/badge";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { store } from "~/lib/data";

export default function Rules() {
  const r = store.rules;
  const primary = () => store.state()?.project.primary;
  return (
    <Page
      title="Rules"
      subtitle="What is measured, and what a pull request must pass before it merges. Everything here is enforced by CI; no maintainer decides."
    >
      <Show
        when={r()}
        fallback={
          <div class="rounded-lg border border-dashed border-border">
            <Empty>Loading rules…</Empty>
          </div>
        }
      >
        {(rules) => (
          <div class="grid gap-6 lg:grid-cols-5">
            <div class="flex flex-col gap-6 lg:col-span-3">
              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>Gates</CardTitle>
                  <span class="text-xs text-faint">all must pass</span>
                </CardHeader>
                <ol class="divide-y divide-border">
                  <For each={rules().gates}>
                    {(g, i) => (
                      <li class="flex gap-4 px-5 py-4">
                        <span class="mt-px w-5 shrink-0 font-mono text-xs leading-5 text-faint tnum">
                          {String(i() + 1).padStart(2, "0")}
                        </span>
                        <div>
                          <div class="text-sm font-medium">{g.label}</div>
                          <p class="mt-1 text-sm leading-relaxed text-muted-foreground">
                            {g.description}
                          </p>
                        </div>
                      </li>
                    )}
                  </For>
                </ol>
              </Card>

              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>Banned constructs</CardTitle>
                </CardHeader>
                <Show when={rules().deny.length} fallback={<Empty>None configured.</Empty>}>
                  <ul class="divide-y divide-border">
                    <For each={rules().deny}>
                      {(d) => (
                        <li class="flex flex-col gap-1.5 px-5 py-3 sm:flex-row sm:items-baseline sm:gap-4">
                          <code class="w-44 shrink-0 font-mono text-[13px]">{d.name}</code>
                          <span class="flex-1 text-sm text-muted-foreground">{d.description}</span>
                          <Badge variant="outline" class="self-start">
                            {d.mode}
                          </Badge>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
              </Card>
            </div>

            <div class="flex flex-col gap-6 lg:col-span-2">
              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>Metrics</CardTitle>
                </CardHeader>
                <ul class="divide-y divide-border">
                  <For each={rules().metrics}>
                    {(m) => (
                      <li class="px-5 py-3.5">
                        <div class="flex items-center gap-2 text-sm font-medium">
                          {m.label}
                          <Show when={m.key === primary()}>
                            <Badge variant="good">primary</Badge>
                          </Show>
                          <span class="ml-auto font-mono text-xs font-normal text-faint">{m.unit}</span>
                        </div>
                        <p class="mt-1 text-sm leading-relaxed text-muted-foreground">
                          {m.description}
                        </p>
                      </li>
                    )}
                  </For>
                </ul>
              </Card>

              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>Frozen paths</CardTitle>
                  <span class="text-xs text-faint">may not change</span>
                </CardHeader>
                <ul class="px-5 py-3.5">
                  <For each={rules().frozen}>
                    {(f) => <li class="py-0.5 font-mono text-[13px] break-all">{f}</li>}
                  </For>
                </ul>
              </Card>

              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>Settings</CardTitle>
                </CardHeader>
                <dl class="divide-y divide-border">
                  <For each={Object.entries(rules().settings)}>
                    {([k, v]) => (
                      <div class="flex items-baseline justify-between gap-4 px-5 py-2.5">
                        <dt class="font-mono text-xs text-muted-foreground">{k}</dt>
                        <dd class="font-mono text-[13px] tnum">{String(v)}</dd>
                      </div>
                    )}
                  </For>
                </dl>
              </Card>
            </div>
          </div>
        )}
      </Show>
    </Page>
  );
}
