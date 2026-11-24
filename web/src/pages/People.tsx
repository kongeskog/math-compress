import { createMemo, For, Show } from "solid-js";
import { IconArrowUpRight } from "~/components/icons";
import { Empty, Page } from "~/components/Page";
import { Badge } from "~/components/ui/badge";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { store } from "~/lib/data";
import { ago, num } from "~/lib/format";
import type { Member, Role } from "~/lib/types";

const GROUPS: { role: Role; title: string; note: string; empty: string }[] = [
  { role: "leader", title: "Leaders", note: "grant roles, review, triage", empty: "No leaders listed yet." },
  { role: "reviewer", title: "Reviewers", note: "their approval merges a change", empty: "No vetted reviewers yet; leaders review." },
  { role: "ideator", title: "Ideators", note: "their ideas go straight into the queue", empty: "No vetted ideators yet; leaders triage." },
];

function MemberRow(props: { m: Member }) {
  return (
    <li class="flex items-center gap-3 px-5 py-3">
      <a
        href={`https://github.com/${props.m.login}`}
        target="_blank"
        rel="noreferrer"
        class="min-w-0 flex-1 truncate text-sm hover:underline"
      >
        @{props.m.login}
      </a>
      <span class="shrink-0 text-right text-xs text-muted-foreground">
        <Show when={props.m.by?.length} fallback="founding">
          endorsed by {props.m.by.map((b) => `@${b}`).join(", ")}
        </Show>
        <span class="text-faint"> · {ago(props.m.since, store.now())}</span>
      </span>
    </li>
  );
}

export default function People() {
  const members = (role: Role) => (store.roles()?.members ?? []).filter((m) => m.role === role);
  const applications = createMemo(() =>
    (store.roles()?.applications ?? []).filter((a) => a.status === "open"),
  );
  const rr = () => store.rules()?.roles;
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

  return (
    <Page
      title="People"
      subtitle="Anyone can contribute changes. Reviewing, and putting ideas straight into the queue, are earned: the leaders vet each person on their record."
    >
      <div class="grid gap-6 lg:grid-cols-5">
        <div class="flex flex-col gap-6 lg:col-span-3">
          <For each={GROUPS}>
            {(g) => (
              <Card class="overflow-hidden">
                <CardHeader>
                  <CardTitle>
                    {g.title} <span class="font-normal text-faint tnum">{members(g.role).length}</span>
                  </CardTitle>
                  <span class="text-xs text-faint">{g.note}</span>
                </CardHeader>
                <Show when={members(g.role).length} fallback={<Empty>{g.empty}</Empty>}>
                  <ul class="divide-y divide-border">
                    <For each={members(g.role)}>{(m) => <MemberRow m={m} />}</For>
                  </ul>
                </Show>
              </Card>
            )}
          </For>
        </div>

        <div class="flex flex-col gap-6 lg:col-span-2">
          <Card class="overflow-hidden">
            <CardHeader>
              <CardTitle>Open applications</CardTitle>
              <span class="text-xs text-faint">leaders decide</span>
            </CardHeader>
            <Show when={applications().length} fallback={<Empty>No open applications.</Empty>}>
              <ul class="divide-y divide-border">
                <For each={applications()}>
                  {(a) => (
                    <li class="px-5 py-3.5">
                      <div class="flex items-center gap-2 text-sm">
                        <a
                          href={`https://github.com/${a.login}`}
                          target="_blank"
                          rel="noreferrer"
                          class="min-w-0 truncate hover:underline"
                        >
                          @{a.login}
                        </a>
                        <Badge variant="outline">{a.role}</Badge>
                        <a
                          href={a.url}
                          target="_blank"
                          rel="noreferrer"
                          class="ml-auto inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
                        >
                          #{a.issue}
                          <IconArrowUpRight width={12} height={12} />
                        </a>
                      </div>
                      <div class="mt-1.5 text-xs text-muted-foreground tnum">
                        {plural(a.merged, "merged change")} · {num(a.saved)} tokens saved ·{" "}
                        {a.endorsements.length} of {plural(a.needed, "endorsement")} ·{" "}
                        {ago(a.opened_at, store.now())}
                      </div>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Card>

          <Card class="overflow-hidden">
            <CardHeader>
              <CardTitle>How roles work</CardTitle>
            </CardHeader>
            <ul class="divide-y divide-border text-sm leading-relaxed text-muted-foreground">
              <li class="px-5 py-3.5">
                <span class="font-medium text-foreground">Contributor.</span> Anyone. Open a pull request;
                the gates and a vetted reviewer decide.
              </li>
              <li class="px-5 py-3.5">
                <span class="font-medium text-foreground">Reviewer.</span>{" "}
                <Show when={rr()} fallback="Earned by merged changes and a leader's endorsement.">
                  {(r) => (
                    <>
                      After {plural(r().reviewer_min_merged, "merged change")}, comment{" "}
                      <code class="font-mono text-[13px] text-foreground">/apply reviewer</code>;{" "}
                      {plural(r().endorsements, "leader endorsement")} grants it.
                    </>
                  )}
                </Show>
              </li>
              <li class="px-5 py-3.5">
                <span class="font-medium text-foreground">Ideator.</span> Ideas from newcomers are proposed
                until an ideator or leader accepts them.{" "}
                <Show when={rr()}>
                  {(r) => (
                    <>
                      After {plural(r().ideator_min_merged, "merged change")},{" "}
                      <code class="font-mono text-[13px] text-foreground">/apply ideator</code> to file
                      ideas straight into the queue.
                    </>
                  )}
                </Show>
              </li>
              <li class="px-5 py-3.5">
                <span class="font-medium text-foreground">Leader.</span>{" "}
                <Show when={rr()} fallback="Added by the existing leaders.">
                  {(r) => (
                    <>
                      After {plural(r().leader_min_merged, "merged change")},{" "}
                      <code class="font-mono text-[13px] text-foreground">/apply leader</code>; needs{" "}
                      {plural(r().leader_endorsements, "endorsement")} from existing leaders.
                    </>
                  )}
                </Show>{" "}
                Leaders can also grant or revoke reviewer and ideator directly.
              </li>
            </ul>
          </Card>
        </div>
      </div>
    </Page>
  );
}
