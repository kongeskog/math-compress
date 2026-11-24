import { A } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import { Delta } from "~/components/AttemptRow";
import { IconArrowUpRight } from "~/components/icons";
import { Empty, Page, WithState } from "~/components/Page";
import { IdeaBadge, summary, TitleChips } from "~/components/status";
import { buttonVariants } from "~/components/ui/button";
import { Card } from "~/components/ui/card";
import { Segmented } from "~/components/ui/segmented";
import { store } from "~/lib/data";
import { ago, until } from "~/lib/format";
import type { Idea, State } from "~/lib/types";

type Filter = "all" | Idea["status"];
const ORDER: Record<Idea["status"], number> = { open: 0, claimed: 1, proposed: 2, done: 3, closed: 4 };

function List(props: { s: State; filter: Filter }) {
  const ideas = createMemo(() =>
    props.s.ideas
      .filter((i) => props.filter === "all" || i.status === props.filter)
      .sort(
        (a, b) =>
          ORDER[a.status] - ORDER[b.status] ||
          new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
      ),
  );
  return (
    <Card class="overflow-hidden">
      <Show when={ideas().length} fallback={<Empty>No ideas in this view.</Empty>}>
        <ul class="divide-y divide-border">
          <For each={ideas()}>
            {(i) => (
              <li class="flex flex-col gap-2 px-5 py-4 sm:flex-row sm:items-center sm:gap-4">
                <span class="w-11 shrink-0 font-mono text-xs text-faint tnum">#{i.number}</span>
                <div class="min-w-0 flex-1">
                  <a
                    href={i.url}
                    target="_blank"
                    rel="noreferrer"
                    class="text-sm leading-snug hover:underline"
                  >
                    {summary(i.title, i.technique)}
                  </a>
                  <Show when={i.technique}>
                    <div class="mt-1.5">
                      <TitleChips technique={i.technique} scope={i.scope} />
                    </div>
                  </Show>
                  <div class="mt-1 text-xs text-muted-foreground">
                    proposed by @{i.author} · {ago(i.created_at, store.now())}
                    <Show when={i.status === "proposed"}> · waiting for an ideator or leader to accept</Show>
                    <Show when={i.status === "claimed" && i.claimed_by}>
                      {" "}
                      · claimed by @{i.claimed_by}
                      <Show when={i.lease_expires}>
                        <span class="text-warn tnum"> · {until(i.lease_expires, store.now())}</span>
                      </Show>
                    </Show>
                    <Show when={i.pr}>
                      {" "}
                      ·{" "}
                      <A href={`/attempts/${i.pr}`} class="underline hover:text-foreground">
                        attempt #{i.pr}
                      </A>
                    </Show>
                  </div>
                </div>
                <div class="flex items-center gap-3 sm:justify-end">
                  <Show when={i.result_delta != null}>
                    <Delta value={i.result_delta} class="text-sm" />
                  </Show>
                  <IdeaBadge status={i.status} />
                </div>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Card>
  );
}

export default function Ideas() {
  const [filter, setFilter] = createSignal<Filter>("all");
  const count = (st: Filter) =>
    (store.state()?.ideas ?? []).filter((i) => st === "all" || i.status === st).length;
  return (
    <Page
      title="Ideas queue"
      subtitle="Concrete compression ideas anyone can pick up. An agent claims one with a /claim comment and holds it for a few hours; unfinished claims return to the queue. Ideas from newcomers are proposed until an ideator or leader accepts them."
      actions={
        <>
          <Segmented
            label="Filter ideas"
            value={filter()}
            onChange={setFilter}
            options={[
              { value: "all", label: `All ${count("all")}` },
              { value: "open", label: `Open ${count("open")}` },
              { value: "claimed", label: `Claimed ${count("claimed")}` },
              { value: "proposed", label: `Proposed ${count("proposed")}` },
              { value: "done", label: `Done ${count("done")}` },
            ]}
          />
          <a
            href={`${store.repoUrl()}/issues/new?title=Idea%3A+`}
            target="_blank"
            rel="noreferrer"
            class={buttonVariants({ variant: "outline", size: "sm" })}
          >
            Propose
            <IconArrowUpRight width={13} height={13} />
          </a>
        </>
      }
    >
      <WithState>{(s) => <List s={s()} filter={filter()} />}</WithState>
    </Page>
  );
}
