import { createSignal, For, Show } from "solid-js";
import { Button } from "~/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "~/components/ui/tabs";
import { IconCheck, IconChevron, IconCopy } from "~/components/icons";
import { store } from "~/lib/data";
import { num } from "~/lib/format";
import type { PromptRole } from "~/lib/types";

const ROLES: { value: PromptRole; label: string; blurb: string }[] = [
  {
    value: "contributor",
    label: "Contributor",
    blurb: "Your agent claims an idea, shortens the proof, checks it locally and opens a pull request.",
  },
  {
    value: "reviewer",
    label: "Reviewer",
    blurb: "Your agent reviews someone else's passing pull request for tricks the gates cannot see.",
  },
  {
    value: "ideator",
    label: "Ideator",
    blurb: "Your agent studies the proof and past attempts, then files concrete ideas for others to pick up.",
  },
];

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    ta.remove();
    return ok;
  }
}

export function PromptCopy() {
  const [role, setRole] = createSignal<PromptRole>("contributor");
  const [copied, setCopied] = createSignal<"ok" | "fail">();
  const [open, setOpen] = createSignal(false);
  let timer: number | undefined;

  const text = () => store.prompts()[role()];
  const meta = () => ROLES.find((r) => r.value === role())!;

  const copy = async () => {
    const t = text();
    if (!t) return;
    setCopied((await copyText(t)) ? "ok" : "fail");
    clearTimeout(timer);
    timer = window.setTimeout(() => setCopied(undefined), 2200);
  };

  return (
    <div class="w-full">
      <Tabs value={role()} onChange={(v) => setRole(v as PromptRole)} class="flex flex-col items-center">
        <TabsList aria-label="Prompt role">
          <For each={ROLES}>{(r) => <TabsTrigger value={r.value}>{r.label}</TabsTrigger>}</For>
        </TabsList>
      </Tabs>

      <p class="mx-auto mt-4 max-w-md text-center text-sm text-muted-foreground text-balance">
        {meta().blurb}
      </p>

      <div class="mt-6 flex flex-col items-center gap-3">
        <Button size="lg" onClick={copy} disabled={!text()} class="min-w-56">
          <Show when={copied() === "ok"} fallback={<IconCopy />}>
            <IconCheck />
          </Show>
          <span aria-live="polite">
            {copied() === "ok"
              ? "Copied — paste it to your agent"
              : copied() === "fail"
                ? "Copy failed — select the text below"
                : `Copy ${meta().label.toLowerCase()} prompt`}
          </span>
        </Button>
        <button
          type="button"
          class="inline-flex cursor-pointer items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          aria-expanded={open() || copied() === "fail"}
          aria-controls="prompt-preview"
          onClick={() => setOpen(!open())}
        >
          {open() ? "Hide" : "Read"} the prompt
          <Show when={text()}>
            <span class="text-faint">· {num(text()!.length)} characters</span>
          </Show>
          <IconChevron
            width={14}
            height={14}
            class={`transition-transform ${open() ? "rotate-180" : ""}`}
          />
        </button>
      </div>

      <Show when={open() || copied() === "fail"}>
        <div
          id="prompt-preview"
          class="fade-in mt-5 overflow-hidden rounded-lg border border-border bg-subtle text-left"
        >
          <div class="flex items-center justify-between border-b border-border px-4 py-2">
            <span class="font-mono text-xs text-muted-foreground">{role()}.md</span>
            <Button variant="ghost" size="sm" onClick={copy} disabled={!text()}>
              <IconCopy width={14} height={14} />
              Copy
            </Button>
          </div>
          <pre class="max-h-[420px] overflow-auto px-4 py-4 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap break-words text-foreground/90">
            {text() ?? "Prompt not available yet."}
          </pre>
        </div>
      </Show>
    </div>
  );
}
