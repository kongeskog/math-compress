import { A, HashRouter, Route, type RouteSectionProps } from "@solidjs/router";
import { createEffect, For, lazy, Show } from "solid-js";
import { IconGithub } from "~/components/icons";
import { LiveIndicator } from "~/components/LiveIndicator";
import { ThemeToggle } from "~/components/ThemeToggle";
import { store } from "~/lib/data";
import Landing from "~/pages/Landing";

const Dashboard = lazy(() => import("~/pages/Dashboard"));
const Ideas = lazy(() => import("~/pages/Ideas"));
const Attempts = lazy(() => import("~/pages/Attempts"));
const AttemptDetail = lazy(() => import("~/pages/AttemptDetail"));
const Rules = lazy(() => import("~/pages/Rules"));
const People = lazy(() => import("~/pages/People"));
const Og = lazy(() => import("~/pages/Og"));

const NAV = [
  { href: "/dashboard", label: "Dashboard" },
  { href: "/attempts", label: "Attempts" },
  { href: "/ideas", label: "Ideas" },
  { href: "/people", label: "People" },
  { href: "/rules", label: "Rules" },
];

function Logo() {
  return (
    <svg width="22" height="22" viewBox="0 0 32 32" aria-hidden="true">
      <rect width="32" height="32" rx="7" fill="var(--foreground)" />
      <path
        d="M6 9h6v6h6v6h8"
        fill="none"
        stroke="var(--background)"
        stroke-width="2.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

function Shell(props: RouteSectionProps) {
  createEffect(() => {
    const t = store.state()?.project.title;
    document.title = t ? `${t} · Hillclimb` : "Hillclimb";
  });
  createEffect(() => {
    void props.location.pathname;
    window.scrollTo(0, 0);
  });

  const navLink = (n: (typeof NAV)[number]) => (
    <A
      href={n.href}
      class="rounded-md px-2.5 py-1.5 text-sm whitespace-nowrap text-muted-foreground transition-colors hover:text-foreground"
      activeClass="!text-foreground bg-muted"
    >
      {n.label}
    </A>
  );

  // The link-preview card is rendered bare, without the site chrome.
  if (props.location.pathname === "/og") return <>{props.children}</>;
  return (
    <div class="flex min-h-dvh flex-col">
      <a
        href="#main"
        class="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-sm focus:text-primary-foreground"
        onClick={(e) => {
          e.preventDefault();
          document.getElementById("main")?.focus();
        }}
      >
        Skip to content
      </a>
      <header class="sticky top-0 z-40 border-b border-border bg-background/80 backdrop-blur-md">
        <div class="mx-auto flex h-14 max-w-6xl items-center gap-4 px-4 sm:px-6">
          <A href="/" class="flex shrink-0 items-center gap-2.5" aria-label="Hillclimb home">
            <Logo />
            <span class="text-sm font-semibold tracking-tight">Hillclimb</span>
            <Show when={store.state()}>
              {(s) => (
                <>
                  <span class="text-border-strong" aria-hidden="true">
                    /
                  </span>
                  <span class="max-w-[9rem] truncate text-sm text-muted-foreground sm:max-w-none">
                    {s().project.slug}
                  </span>
                </>
              )}
            </Show>
          </A>
          <nav class="ml-2 hidden items-center gap-0.5 md:flex" aria-label="Main">
            <For each={NAV}>{navLink}</For>
          </nav>
          <div class="ml-auto flex items-center gap-1.5">
            <LiveIndicator />
            <a
              href={store.repoUrl()}
              target="_blank"
              rel="noreferrer"
              class="ml-2 inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              aria-label="GitHub repository"
            >
              <IconGithub />
            </a>
            <ThemeToggle />
          </div>
        </div>
        <nav
          class="flex items-center gap-0.5 overflow-x-auto border-t border-border px-3 py-1.5 md:hidden"
          aria-label="Main"
        >
          <For each={NAV}>{navLink}</For>
        </nav>
      </header>

      <main id="main" tabindex="-1" class="flex-1 outline-none">
        {props.children}
      </main>

      <footer class="border-t border-border">
        <div class="mx-auto flex max-w-6xl flex-col gap-2 px-4 py-8 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between sm:px-6">
          <span>
            Every merge is machine-checked: same theorems, same axioms, fewer tokens.
          </span>
          <span class="flex items-center gap-4">
            <a class="hover:text-foreground" href={store.repoUrl()} target="_blank" rel="noreferrer">
              Proof repository
            </a>
            <Show when={store.config()?.harnessRepo}>
              {(h) => (
                <a
                  class="hover:text-foreground"
                  href={`https://github.com/${h()}`}
                  target="_blank"
                  rel="noreferrer"
                >
                  Run your own hillclimb
                </a>
              )}
            </Show>
          </span>
        </div>
      </footer>
    </div>
  );
}

function NotFound() {
  return (
    <div class="mx-auto max-w-6xl px-4 py-24 text-center sm:px-6">
      <p class="font-mono text-sm text-muted-foreground">404</p>
      <h1 class="mt-2 text-2xl font-semibold tracking-tight">Nothing here</h1>
      <A href="/" class="mt-6 inline-block text-sm underline">
        Back to the start
      </A>
    </div>
  );
}

export default function App() {
  return (
    <HashRouter root={Shell}>
      <Route path="/" component={Landing} />
      <Route path="/dashboard" component={Dashboard} />
      <Route path="/ideas" component={Ideas} />
      <Route path="/attempts" component={Attempts} />
      <Route path="/attempts/:pr" component={AttemptDetail} />
      <Route path="/rules" component={Rules} />
      <Route path="/people" component={People} />
      <Route path="/og" component={Og} />
      <Route path="*" component={NotFound} />
    </HashRouter>
  );
}
