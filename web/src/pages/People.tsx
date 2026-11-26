import { createMemo, For, Show } from "solid-js";
import { IconArrowUpRight } from "~/components/icons";
import { Empty, Page } from "~/components/Page";
import { Badge } from "~/components/ui/badge";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { store } from "~/lib/data";
import { ago, num, pct } from "~/lib/format";
import type { Member, Role } from "~/lib/types";

type Group = { role: Role; title: string; note: string; empty: string };
/** The roles shown, as the arena's review setting has them. */
const groups = (reviewOn: boolean): Group[] => [
  {
    role: "leader",
    title: "Leaders",
    note: reviewOn ? "grant roles, review, triage" : "grant roles, triage, can stop a change",
    empty: "No leaders listed yet.",
  },
  {
    role: "reviewer",
    title: "Reviewers",
    note: reviewOn ? "their approval merges a change" : "can stop a change; no approval is needed here",
    empty: "No vetted reviewers yet; leaders review.",
  },
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
  const board = createMemo(() => [...(store.state()?.leaderboard ?? [])].sort((a, b) => b.saved - a.saved));
  const total = createMemo(() => board().reduce((n, l) => n + l.saved, 0));
  const rolesOf = (login: string) =>
    (store.roles()?.members ?? []).filter((m) => m.login === login).map((m) => m.role);
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const reviewOn = store.reviewOn;
  // Where no change needs an approval, the reviewer group is shown only if
  // someone holds the role (they can still request changes).
  const shown = createMemo(() =>
    groups(reviewOn()).filter((g) => g.role !== "reviewer" || reviewOn() || members("reviewer").length > 0),
  );

  return (
    <Page
      title="People"
      subtitle={
        reviewOn()
          ? "Anyone can contribute changes. Reviewing, and putting ideas straight into the queue, are earned: the leaders vet each person on their record."
          : "Anyone can contribute changes, and a change whose gates all pass merges without anyone's approval. Putting ideas straight into the queue is earned: the leaders vet each person on their record."
      }
    >
      <Card class="mb-6 overflow-hidden">
        <CardHeader>
          <CardTitle>
            Contributors <span class="font-normal text-faint tnum">{board().length}</span>
          </CardTitle>
          <span class="text-xs text-faint">everyone with a change, by tokens saved</span>
        </CardHeader>
        <Show when={board().length} fallback={<Empty>Nobody has opened a change yet.</Empty>}>
          <ol class="divide-y divide-border">
            <For each={board()}>
              {(l, i) => (
                <li class="flex items-center gap-3 px-5 py-3 text-sm">
                  <span class="w-5 shrink-0 font-mono text-xs text-faint tnum">{i() + 1}</span>
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                      <a
                        href={`https://github.com/${l.login}`}
                        target="_blank"
                        rel="noreferrer"
                        class="min-w-0 truncate hover:underline"
                      >
                        @{l.login}
                      </a>
                      <For each={rolesOf(l.login)}>{(r) => <Badge variant="outline">{r}</Badge>}</For>
                    </div>
                    <div class="mt-0.5 text-xs text-muted-foreground tnum sm:hidden">
                      {plural(l.merged, "merged change")} of {l.attempts} ·{" "}
                      {pct((100 * l.saved) / Math.max(1, total()), 1)}
                    </div>
                  </div>
                  <a
                    href={`${store.repoUrl()}/pulls?q=is%3Apr+is%3Amerged+author%3A${l.login}`}
                    target="_blank"
                    rel="noreferrer"
                    class="hidden shrink-0 text-xs text-muted-foreground hover:text-foreground tnum sm:block"
                  >
                    {plural(l.merged, "merged change")} of {l.attempts}
                  </a>
                  <span class="hidden w-28 shrink-0 sm:block" aria-hidden="true">
                    <span
                      class="block h-1.5 rounded-full bg-foreground/70"
                      style={{ width: `${Math.max(2, (100 * l.saved) / Math.max(1, total()))}%` }}
                    />
                  </span>
                  <span class="hidden w-14 shrink-0 text-right text-xs text-faint tnum sm:block">
                    {pct((100 * l.saved) / Math.max(1, total()), 1)}
                  </span>
                  <span class="w-24 shrink-0 text-right font-mono text-good tnum">−{num(l.saved)}</span>
                </li>
              )}
            </For>
          </ol>
        </Show>
      </Card>

      <div class="grid gap-6 lg:grid-cols-5">
        <div class="flex flex-col gap-6 lg:col-span-3">
          <For each={shown()}>
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
                <span class="font-medium text-foreground">Contributor.</span> Anyone. Open a pull request;{" "}
                {reviewOn() ? "the gates and a vetted reviewer decide." : "the gates decide, and nobody has to approve it."}
              </li>
              <Show when={reviewOn()}>
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
              </Show>
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
                {reviewOn()
                  ? "Leaders can also grant or revoke reviewer and ideator directly."
                  : "Leaders can also grant or revoke ideator directly, and can stop a change that passes the gates by requesting changes on it."}
              </li>
            </ul>
          </Card>
        </div>
      </div>
    </Page>
  );
}
