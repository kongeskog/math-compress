import { createMemo, createSignal, For, Show } from "solid-js";
import { AttemptRow } from "~/components/AttemptRow";
import { Empty, Page, WithState } from "~/components/Page";
import { Card } from "~/components/ui/card";
import { Segmented } from "~/components/ui/segmented";
import { store } from "~/lib/data";

type Filter = "all" | "open" | "merged" | "rejected";

export default function Attempts() {
  const [filter, setFilter] = createSignal<Filter>("all");
  const match = (f: Filter) =>
    store.all().filter((a) =>
      f === "all"
        ? true
        : f === "open"
          ? a.status !== "merged" && a.status !== "rejected"
          : a.status === f,
    );
  const [technique, setTechnique] = createSignal("");
  const techniques = createMemo(() => {
    const seen = new Set<string>();
    for (const t of store.rules()?.techniques ?? []) seen.add(t.key);
    for (const a of store.all()) if (a.technique) seen.add(a.technique);
    return [...seen];
  });
  const rows = createMemo(() =>
    match(filter()).filter((a) => !technique() || a.technique === technique()),
  );
  return (
    <Page
      title="Attempts"
      subtitle="Every pull request, kept or not. Rejected attempts stay on the record so nobody repeats them."
      actions={
        <>
        <Show when={techniques().length}>
          <select
            aria-label="Filter by technique"
            class="h-8 rounded-md border border-border bg-background px-2 text-sm text-foreground"
            value={technique()}
            onChange={(e) => setTechnique(e.currentTarget.value)}
          >
            <option value="">All techniques</option>
            <For each={techniques()}>{(t) => <option value={t}>{t}</option>}</For>
          </select>
        </Show>
        <Segmented
          label="Filter attempts"
          value={filter()}
          onChange={setFilter}
          options={[
            { value: "all", label: `All ${match("all").length}` },
            { value: "open", label: `In flight ${match("open").length}` },
            { value: "merged", label: `Merged ${match("merged").length}` },
            { value: "rejected", label: `Rejected ${match("rejected").length}` },
          ]}
        />
        </>
      }
    >
      <WithState>
        {() => (
          <Card class="overflow-hidden">
            <Show when={rows().length} fallback={<Empty>No attempts in this view.</Empty>}>
              <div class="divide-y divide-border">
                <For each={rows()}>{(a) => <AttemptRow a={a} />}</For>
              </div>
            </Show>
          </Card>
        )}
      </WithState>
    </Page>
  );
}
