import { Show } from "solid-js";
import { store } from "~/lib/data";
import { ago } from "~/lib/format";

export function LiveIndicator() {
  const stale = () => !!store.error();
  const label = () => {
    const f = store.fetchedAt();
    if (!f) return stale() ? "offline" : "connecting";
    return `updated ${ago(new Date(f).toISOString(), store.now())}`;
  };
  return (
    <span
      class="inline-flex items-center gap-2 text-xs text-muted-foreground tnum"
      title={store.error() ? `Last refresh failed: ${store.error()}` : "Refreshes every 60 seconds"}
      aria-live="off"
    >
      <span class="relative flex size-2">
        <Show when={!stale()}>
          <span class="live-ping absolute inline-flex h-full w-full rounded-full bg-good" />
        </Show>
        <span
          class="relative inline-flex size-2 rounded-full"
          classList={{ "bg-good": !stale(), "bg-warn": stale() }}
        />
      </span>
      <span class="hidden sm:inline">{label()}</span>
      <span class="sm:hidden">{stale() ? "offline" : "live"}</span>
    </span>
  );
}
