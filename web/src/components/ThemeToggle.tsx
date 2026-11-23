import { createSignal, Show } from "solid-js";
import { Button } from "~/components/ui/button";
import { IconMoon, IconSun } from "~/components/icons";

export function ThemeToggle() {
  const [dark, setDark] = createSignal(document.documentElement.classList.contains("dark"));
  const toggle = () => {
    const next = !dark();
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
    try {
      localStorage.setItem("hc-theme", next ? "dark" : "light");
    } catch {
      // Private mode: the choice just does not persist.
    }
  };
  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={toggle}
      aria-label={dark() ? "Switch to light theme" : "Switch to dark theme"}
    >
      <Show when={dark()} fallback={<IconMoon />}>
        <IconSun />
      </Show>
    </Button>
  );
}
