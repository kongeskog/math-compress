import { createSignal, Show } from "solid-js";
import { Button } from "~/components/ui/button";
import { IconCheck, IconCopy } from "~/components/icons";
import { store } from "~/lib/data";

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
  const [copied, setCopied] = createSignal<"ok" | "fail">();
  let timer: number | undefined;

  const text = () => store.prompts().invite?.trim();

  const copy = async () => {
    const t = text();
    if (!t) return;
    setCopied((await copyText(t)) ? "ok" : "fail");
    clearTimeout(timer);
    timer = window.setTimeout(() => setCopied(undefined), 2200);
  };

  return (
    <div class="w-full">
      <p class="mx-auto max-w-md text-center text-sm text-muted-foreground text-balance">
        Paste this to the coding agent you already use. It reads the instructions, sets up, picks an idea and
        opens a pull request.
      </p>

      <div class="mx-auto mt-5 max-w-xl rounded-lg border border-border bg-subtle px-4 py-3 text-left">
        <p class="font-mono text-[13px] leading-relaxed break-words text-foreground/90 select-all">
          {text() ?? "Loading…"}
        </p>
      </div>

      <div class="mt-5 flex flex-col items-center gap-3">
        <Button size="lg" onClick={copy} disabled={!text()} class="min-w-56">
          <Show when={copied() === "ok"} fallback={<IconCopy />}>
            <IconCheck />
          </Show>
          <span aria-live="polite">
            {copied() === "ok"
              ? "Copied — paste it to your agent"
              : copied() === "fail"
                ? "Copy failed — select the text above"
                : "Copy prompt"}
          </span>
        </Button>
        <a
          href="./join.txt"
          target="_blank"
          rel="noreferrer"
          class="text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
        >
          Read the full instructions (join.txt)
        </a>
      </div>
    </div>
  );
}
