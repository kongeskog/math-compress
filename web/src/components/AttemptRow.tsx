import { A } from "@solidjs/router";
import { Show } from "solid-js";
import { AttemptBadge, summary, TitleChips } from "~/components/status";
import { store } from "~/lib/data";
import { ago, signed } from "~/lib/format";
import type { Attempt } from "~/lib/types";

export function Delta(props: { value: number | null | undefined; class?: string }) {
  return (
    <span
      class={`font-mono tnum ${props.class ?? ""}`}
      classList={{
        "text-good": (props.value ?? 0) < 0,
        "text-bad": (props.value ?? 0) > 0,
        "text-faint": props.value == null || props.value === 0,
      }}
    >
      {signed(props.value)}
    </span>
  );
}

/** One line in a feed or list. */
export function AttemptRow(props: { a: Attempt; dense?: boolean }) {
  return (
    <A
      href={`/attempts/${props.a.pr}`}
      class="group flex items-center gap-3 px-5 py-3 transition-colors hover:bg-subtle"
    >
      <span class="w-11 shrink-0 font-mono text-xs text-faint tnum">#{props.a.pr}</span>
      <div class="min-w-0 flex-1">
        <div class="flex min-w-0 items-center gap-2 text-sm">
          <TitleChips technique={props.a.technique} scope={props.a.scope} class="hidden shrink-0 sm:inline-flex" />
          <span class="truncate">{summary(props.a.title, props.a.technique)}</span>
        </div>
        <div class="mt-0.5 truncate text-xs text-muted-foreground">
          @{props.a.author} · {ago(props.a.decided_at ?? props.a.created_at, store.now())}
          <Show when={props.a.status === "rejected" && props.a.reason}>
            {" "}
            · {props.a.reason}
          </Show>
        </div>
      </div>
      <Show when={props.a.delta != null && props.a.status !== "rejected"}>
        <Delta value={props.a.delta} class="hidden text-sm sm:inline" />
      </Show>
      <Show when={!props.dense}>
        <AttemptBadge status={props.a.status} />
      </Show>
      <Show when={props.dense}>
        <span
          class="size-2 shrink-0 rounded-full"
          classList={{
            "bg-good": props.a.status === "merged",
            "bg-faint": props.a.status === "rejected",
            "bg-warn": props.a.status !== "merged" && props.a.status !== "rejected",
          }}
          title={props.a.status.replace("_", " ")}
        />
      </Show>
    </A>
  );
}
