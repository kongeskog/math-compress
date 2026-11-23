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
  const rows = createMemo(() => match(filter()));
  return (
    <Page
      title="Attempts"
      subtitle="Every pull request, kept or not. Rejected attempts stay on the record so nobody repeats them."
      actions={
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
