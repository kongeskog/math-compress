import { For } from "solid-js";
import { cn } from "~/lib/cn";

export function Segmented<T extends string>(props: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
  class?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={props.label}
      class={cn(
        "inline-flex h-8 max-w-full items-center overflow-x-auto rounded-md border border-border bg-subtle p-0.5",
        props.class,
      )}
    >
      <For each={props.options}>
        {(o) => (
          <button
            type="button"
            role="radio"
            aria-checked={props.value === o.value}
            onClick={() => props.onChange(o.value)}
            class={cn(
              "h-full cursor-pointer rounded-[5px] px-2.5 text-xs font-medium whitespace-nowrap text-muted-foreground transition-colors hover:text-foreground",
              props.value === o.value &&
                "bg-background text-foreground shadow-[0_0_0_1px_var(--border)]",
            )}
          >
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}
