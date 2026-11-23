import { Tabs as K } from "@kobalte/core/tabs";
import { splitProps, type ComponentProps } from "solid-js";
import { cn } from "~/lib/cn";

export const Tabs = K;

export function TabsList(props: ComponentProps<typeof K.List>) {
  const [local, rest] = splitProps(props, ["class"]);
  return (
    <K.List
      class={cn(
        "relative inline-flex h-9 items-center rounded-md border border-border bg-subtle p-0.5",
        local.class,
      )}
      {...rest}
    />
  );
}

export function TabsTrigger(props: ComponentProps<typeof K.Trigger>) {
  const [local, rest] = splitProps(props, ["class"]);
  return (
    <K.Trigger
      class={cn(
        "inline-flex h-full cursor-pointer items-center justify-center rounded-[5px] px-3 text-[13px] font-medium text-muted-foreground transition-colors hover:text-foreground data-[selected]:bg-background data-[selected]:text-foreground data-[selected]:shadow-[0_0_0_1px_var(--border)]",
        local.class,
      )}
      {...rest}
    />
  );
}

export function TabsContent(props: ComponentProps<typeof K.Content>) {
  const [local, rest] = splitProps(props, ["class"]);
  return <K.Content class={cn("mt-4", local.class)} {...rest} />;
}
