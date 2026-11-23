import { Show, type JSX } from "solid-js";
import { store } from "~/lib/data";
import type { State } from "~/lib/types";

export function Page(props: { title: string; subtitle?: JSX.Element; actions?: JSX.Element; children: JSX.Element }) {
  return (
    <div class="mx-auto max-w-6xl px-4 py-8 sm:px-6 sm:py-10">
      <div class="mb-8 flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
        <div class="min-w-0">
          <h1 class="text-2xl font-semibold tracking-tight">{props.title}</h1>
          <Show when={props.subtitle}>
            <p class="mt-1.5 max-w-2xl text-sm text-muted-foreground">{props.subtitle}</p>
          </Show>
        </div>
        <Show when={props.actions}>
          <div class="flex shrink-0 flex-wrap items-center gap-2">{props.actions}</div>
        </Show>
      </div>
      {props.children}
    </div>
  );
}

/** Renders children once state.json has loaded; otherwise a quiet loading or error note. */
export function WithState(props: { children: (s: () => State) => JSX.Element }) {
  return (
    <Show
      when={store.state()}
      fallback={
        <div class="rounded-lg border border-dashed border-border px-6 py-16 text-center text-sm text-muted-foreground">
          <Show when={store.error()} fallback="Loading the latest scores…">
            <p class="text-foreground">Could not load data.</p>
            <p class="mt-1 font-mono text-xs">{store.error()}</p>
            <p class="mt-3">Retrying every minute.</p>
          </Show>
        </div>
      }
    >
      {(s) => props.children(s)}
    </Show>
  );
}

export function Empty(props: { children: JSX.Element }) {
  return <div class="px-5 py-10 text-center text-sm text-muted-foreground">{props.children}</div>;
}
