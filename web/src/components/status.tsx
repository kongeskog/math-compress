import { Match, Show, Switch } from "solid-js";
import { Badge } from "~/components/ui/badge";
import { IconCheck, IconMinus, IconSpinner, IconX } from "~/components/icons";
import type { AttemptStatus, GateStatus, Idea } from "~/lib/types";

const ATTEMPT: Record<AttemptStatus, { label: string; variant: "good" | "warn" | "bad" | "neutral" }> = {
  merged: { label: "Merged", variant: "good" },
  rejected: { label: "Rejected", variant: "neutral" },
  verifying: { label: "Verifying", variant: "warn" },
  awaiting_review: { label: "Awaiting review", variant: "warn" },
  stale: { label: "Re-verifying", variant: "warn" },
  conflict: { label: "Conflict", variant: "bad" },
  needs_fix: { label: "Needs a fix", variant: "warn" },
};

export function AttemptBadge(props: { status: AttemptStatus }) {
  const s = () => ATTEMPT[props.status] ?? { label: props.status, variant: "neutral" as const };
  return (
    <Badge variant={s().variant}>
      <span class="size-1.5 rounded-full bg-current" />
      {s().label}
    </Badge>
  );
}

const IDEA: Record<Idea["status"], { label: string; variant: "good" | "warn" | "neutral" | "outline" }> = {
  proposed: { label: "Proposed", variant: "neutral" },
  open: { label: "Open", variant: "outline" },
  claimed: { label: "Claimed", variant: "warn" },
  done: { label: "Done", variant: "good" },
  closed: { label: "Closed", variant: "neutral" },
};

export function IdeaBadge(props: { status: Idea["status"] }) {
  const s = () => IDEA[props.status] ?? { label: props.status, variant: "neutral" as const };
  return <Badge variant={s().variant}>{s().label}</Badge>;
}

/** `technique` and `scope` from a standard title, as small mono chips. */
export function TitleChips(props: { technique?: string | null; scope?: string | null; class?: string }) {
  return (
    <Show when={props.technique}>
      <span class={`inline-flex min-w-0 items-center gap-1 align-middle ${props.class ?? ""}`}>
        <span class="shrink-0 rounded border border-border bg-muted px-1.5 py-px font-mono text-[11px] leading-4 text-muted-foreground">
          {props.technique}
        </span>
        <Show when={props.scope}>
          <span class="truncate rounded border border-border px-1.5 py-px font-mono text-[11px] leading-4 text-faint">
            {props.scope}
          </span>
        </Show>
      </span>
    </Show>
  );
}

/** Standard titles repeat what the chips show; drop the prefix when chips are present. */
export function summary(title: string, technique?: string | null) {
  if (!technique) return title;
  const m = /^(?:Idea:\s*)?[a-z-]+\([^)]*\):\s*(.+)$/.exec(title.trim());
  return m ? m[1] : title;
}

export function GateIcon(props: { status: GateStatus }) {
  return (
    <span
      class="inline-flex size-5 shrink-0 items-center justify-center rounded-full"
      classList={{
        "bg-good-soft text-good": props.status === "pass",
        "bg-bad-soft text-bad": props.status === "fail",
        "bg-warn-soft text-warn": props.status === "pending",
        "bg-muted text-faint": props.status === "skip",
      }}
      role="img"
      aria-label={props.status}
    >
      <Switch>
        <Match when={props.status === "pass"}>
          <IconCheck width={12} height={12} stroke-width={3} />
        </Match>
        <Match when={props.status === "fail"}>
          <IconX width={12} height={12} stroke-width={3} />
        </Match>
        <Match when={props.status === "pending"}>
          <IconSpinner width={12} height={12} stroke-width={3} />
        </Match>
        <Match when={props.status === "skip"}>
          <IconMinus width={12} height={12} stroke-width={3} />
        </Match>
      </Switch>
    </span>
  );
}
