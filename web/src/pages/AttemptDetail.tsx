import { A, useParams } from "@solidjs/router";
import { createMemo, For, Show } from "solid-js";
import { Delta } from "~/components/AttemptRow";
import { IconArrowUpRight } from "~/components/icons";
import { Empty, WithState } from "~/components/Page";
import { AttemptBadge, GateIcon, TitleChips } from "~/components/status";
import { buttonVariants } from "~/components/ui/button";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { store } from "~/lib/data";
import { dateTime, metricValue, sha } from "~/lib/format";
import type { Attempt, State } from "~/lib/types";

function Detail(props: { a: Attempt; s: State }) {
  const passed = () => props.a.gates.filter((g) => g.status === "pass").length;
  const rows = createMemo(() =>
    props.s.project.metrics.map((m) => {
      const before = props.a.base_metrics?.[m.key] ?? null;
      const after = props.a.metrics?.[m.key] ?? null;
      return { m, before, after, delta: before != null && after != null ? after - before : null };
    }),
  );
  return (
    <>
      <div class="mb-8">
        <A href="/attempts" class="text-xs text-muted-foreground hover:text-foreground">
          ← Attempts
        </A>
        <div class="mt-3 flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2.5">
              <span class="font-mono text-sm text-faint">#{props.a.pr}</span>
              <AttemptBadge status={props.a.status} />
              <Show when={props.a.delta != null}>
                <Delta value={props.a.delta} class="text-sm" />
              </Show>
              <TitleChips technique={props.a.technique} scope={props.a.scope} />
            </div>
            <h1 class="mt-2 text-2xl font-semibold tracking-tight text-balance">{props.a.title}</h1>
            <p class="mt-2 text-sm text-muted-foreground">
              by{" "}
              <a
                class="text-foreground hover:underline"
                href={`https://github.com/${props.a.author}`}
                target="_blank"
                rel="noreferrer"
              >
                @{props.a.author}
              </a>{" "}
              · opened {dateTime(props.a.created_at)}
              <Show when={props.a.decided_at}> · decided {dateTime(props.a.decided_at)}</Show>
            </p>
          </div>
          <div class="flex shrink-0 flex-wrap gap-2">
            <a
              href={props.a.url}
              target="_blank"
              rel="noreferrer"
              class={buttonVariants({ variant: "default", size: "sm" })}
            >
              Pull request
              <IconArrowUpRight width={13} height={13} />
            </a>
            <Show when={props.a.run_url}>
              <a
                href={props.a.run_url!}
                target="_blank"
                rel="noreferrer"
                class={buttonVariants({ variant: "outline", size: "sm" })}
              >
                CI run
                <IconArrowUpRight width={13} height={13} />
              </a>
            </Show>
          </div>
        </div>
        <Show when={props.a.reason}>
          <div
            class="mt-5 rounded-md border px-4 py-3 text-sm"
            classList={{
              "border-border bg-subtle text-muted-foreground": props.a.status === "rejected",
              "border-transparent bg-warn-soft text-warn": props.a.status !== "rejected",
            }}
          >
            {props.a.reason}
          </div>
        </Show>
      </div>

      <div class="grid gap-6 lg:grid-cols-2">
        <Card class="overflow-hidden">
          <CardHeader>
            <CardTitle>Gates</CardTitle>
            <Show when={props.a.gates.length}>
              <span class="text-xs text-muted-foreground tnum">
                {passed()} of {props.a.gates.length} passed
              </span>
            </Show>
          </CardHeader>
          <Show
            when={props.a.gates.length}
            fallback={<Empty>Verification has not reported yet. Static checks take seconds; the full build takes longer.</Empty>}
          >
            <ul class="divide-y divide-border">
              <For each={props.a.gates}>
                {(g) => (
                  <li class="flex items-start gap-3 px-5 py-3">
                    <GateIcon status={g.status} />
                    <div class="min-w-0">
                      <div class="text-sm">{g.label}</div>
                      <Show when={g.detail}>
                        <div class="mt-0.5 text-xs break-words text-muted-foreground">{g.detail}</div>
                      </Show>
                    </div>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Card>

        <div class="flex flex-col gap-6">
          <Card class="overflow-hidden">
            <CardHeader>
              <CardTitle>Metrics</CardTitle>
              <span class="text-xs text-faint">vs. the commit it was verified on</span>
            </CardHeader>
            <Show when={props.a.metrics} fallback={<Empty>Not measured.</Empty>}>
              <div class="overflow-x-auto">
                <table class="w-full text-sm">
                  <thead>
                    <tr class="border-b border-border text-left text-xs text-muted-foreground">
                      <th class="px-5 py-2 font-normal">Metric</th>
                      <th class="px-3 py-2 text-right font-normal">Before</th>
                      <th class="px-3 py-2 text-right font-normal">After</th>
                      <th class="py-2 pr-5 pl-3 text-right font-normal">Change</th>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-border">
                    <For each={rows()}>
                      {(r) => (
                        <tr>
                          <td class="px-5 py-2.5">
                            {r.m.label}
                            <Show when={r.m.key === props.s.project.primary}>
                              <span class="ml-2 text-xs text-faint">primary</span>
                            </Show>
                          </td>
                          <td class="px-3 py-2.5 text-right font-mono text-muted-foreground tnum">
                            {metricValue(r.m.key, r.before)}
                          </td>
                          <td class="px-3 py-2.5 text-right font-mono tnum">
                            {metricValue(r.m.key, r.after)}
                          </td>
                          <td class="py-2.5 pr-5 pl-3 text-right">
                            <Delta value={r.delta} />
                          </td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>
          </Card>

          <Card>
            <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2.5 px-5 py-4 text-sm">
              <dt class="text-muted-foreground">Head</dt>
              <dd class="font-mono text-xs leading-5">{sha(props.a.head_sha)}</dd>
              <dt class="text-muted-foreground">Verified on</dt>
              <dd class="font-mono text-xs leading-5">{sha(props.a.base_sha)}</dd>
              <Show when={props.a.merge_sha}>
                <dt class="text-muted-foreground">Merged as</dt>
                <dd class="font-mono text-xs leading-5">
                  <a
                    class="hover:underline"
                    href={`${store.repoUrl()}/commit/${props.a.merge_sha}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {sha(props.a.merge_sha)}
                  </a>
                </dd>
              </Show>
              <Show when={props.a.idea}>
                <dt class="text-muted-foreground">Idea</dt>
                <dd>
                  <a
                    class="hover:underline"
                    href={`${store.repoUrl()}/issues/${props.a.idea}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    #{props.a.idea}
                  </a>
                </dd>
              </Show>
            </dl>
          </Card>
        </div>
      </div>
    </>
  );
}

export default function AttemptDetail() {
  const params = useParams();
  const attempt = () => store.find(Number(params.pr));
  return (
    <div class="mx-auto max-w-6xl px-4 py-8 sm:px-6 sm:py-10">
      <WithState>
        {(s) => (
          <Show
            when={attempt()}
            fallback={
              <div class="py-16 text-center">
                <p class="text-sm text-muted-foreground">
                  Attempt #{params.pr} is not in the record yet.
                </p>
                <A href="/attempts" class="mt-4 inline-block text-sm underline">
                  All attempts
                </A>
              </div>
            }
          >
            {(a) => <Detail a={a()} s={s()} />}
          </Show>
        )}
      </WithState>
    </div>
  );
}
