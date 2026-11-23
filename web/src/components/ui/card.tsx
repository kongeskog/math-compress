import { splitProps, type JSX } from "solid-js";
import { cn } from "~/lib/cn";

type Div = JSX.HTMLAttributes<HTMLDivElement>;

export function Card(props: Div) {
  const [local, rest] = splitProps(props, ["class"]);
  return <div class={cn("rounded-lg border border-border bg-card", local.class)} {...rest} />;
}

export function CardHeader(props: Div) {
  const [local, rest] = splitProps(props, ["class"]);
  return (
    <div
      class={cn("flex items-center justify-between gap-3 border-b border-border px-5 py-3.5", local.class)}
      {...rest}
    />
  );
}

export function CardTitle(props: Div) {
  const [local, rest] = splitProps(props, ["class"]);
  return <div class={cn("text-sm font-medium tracking-tight", local.class)} {...rest} />;
}

export function CardContent(props: Div) {
  const [local, rest] = splitProps(props, ["class"]);
  return <div class={cn("p-5", local.class)} {...rest} />;
}
