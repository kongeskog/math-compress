import { scaleLinear, scaleLog } from "d3-scale";
import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Page, WithState } from "~/components/Page";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { store } from "~/lib/data";
import { dateTime, num, pct } from "~/lib/format";
import { median, OTHER, yieldOf, type Change, type Group } from "~/lib/yield";

function useWidth() {
  let host!: HTMLDivElement;
  const [width, setWidth] = createSignal(0);
  onMount(() => {
    const ro = new ResizeObserver((e) => setWidth(Math.floor(e[0].contentRect.width)));
    ro.observe(host);
    onCleanup(() => ro.disconnect());
  });
  return { ref: (el: HTMLDivElement) => (host = el), width };
}

function Stat(props: { label: string; value: string; sub?: string; wide?: boolean }) {
  return (
    <div class="bg-card px-5 py-4" classList={{ "col-span-2 md:col-span-1": props.wide }}>
      <div class="text-xs text-muted-foreground">{props.label}</div>
      <div class="mt-1.5 font-mono text-xl font-medium tracking-tight tnum">{props.value}</div>
      <div class="mt-0.5 min-h-4 text-xs text-faint tnum">{props.sub}</div>
    </div>
  );
}

function ShareBar(props: { share: number; class?: string }) {
  return (
    <div class={`h-1.5 overflow-hidden rounded-full bg-muted ${props.class ?? ""}`}>
      <div
        class="h-full rounded-full bg-foreground"
        style={{ width: `${Math.min(100, Math.max(props.share * 100, props.share > 0 ? 1.5 : 0))}%` }}
      />
    </div>
  );
}

const Th = (props: { children: JSX.Element; left?: boolean; class?: string }) => (
  <th class={`py-2.5 font-normal ${props.left ? "px-5 text-left" : "px-3 text-right"} ${props.class ?? ""}`}>
    {props.children}
  </th>
);
const Td = (props: { children: JSX.Element; muted?: boolean; class?: string }) => (
  <td
    class={`px-3 py-2.5 text-right font-mono tnum ${props.muted ? "text-muted-foreground" : ""} ${props.class ?? ""}`}
  >
    {props.children}
  </td>
);

function TechniqueTable(props: { groups: Group[]; total: number; describe: (k: string) => string | undefined }) {
  return (
    <div class="overflow-x-auto">
      <table class="w-full text-xs">
        <thead class="text-faint">
          <tr class="border-b border-border">
            <Th left>Technique</Th>
            <Th>Changes</Th>
            <Th>Tokens saved</Th>
            <Th class="w-[22%] min-w-24 !text-left">Share</Th>
            <Th>Median</Th>
            <Th class="pr-5">Mean</Th>
          </tr>
        </thead>
        <tbody class="divide-y divide-border">
          <For each={props.groups}>
            {(g) => (
              <tr class="hover:bg-subtle">
                <td class="px-5 py-2.5 text-left">
                  <a href={g.key === OTHER ? "#/attempts" : "#/rules"} class="font-mono hover:underline">
                    {g.key}
                  </a>
                  <Show when={props.describe(g.key)}>
                    <div class="mt-0.5 hidden max-w-xs text-faint sm:block">{props.describe(g.key)}</div>
                  </Show>
                </td>
                <Td muted>{num(g.n)}</Td>
                <Td>{num(g.saved)}</Td>
                <td class="px-3 py-2.5">
                  <div class="flex items-center gap-2">
                    <ShareBar share={g.share} class="flex-1" />
                    <span class="w-9 text-right font-mono text-muted-foreground tnum">{pct(g.share * 100, 0)}</span>
                  </div>
                </td>
                <Td muted>{num(g.median)}</Td>
                <Td muted class="pr-5">
                  {num(g.mean)}
                </Td>
              </tr>
            )}
          </For>
        </tbody>
        <tfoot>
          <tr class="border-t border-border text-muted-foreground">
            <td class="px-5 py-2.5">All</td>
            <Td muted>{num(props.groups.reduce((s, g) => s + g.n, 0))}</Td>
            <Td>{num(props.total)}</Td>
            <td />
            <td />
            <td />
          </tr>
        </tfoot>
      </table>
    </div>
  );
}

/** Cumulative share of the reduction against the number of largest changes. */
function Curve(props: { pts: { cum: number; share: number }[]; marks: number[] }) {
  const w = useWidth();
  const H = 200;
  const m = { t: 12, r: 14, b: 22, l: 38 };
  const n = () => props.pts.length;
  const x = createMemo(() => scaleLinear().domain([0, Math.max(1, n())]).range([m.l, Math.max(m.l + 10, w.width() - m.r)]));
  const y = scaleLinear().domain([0, 1]).range([H - m.b, m.t]);
  const path = createMemo(
    () =>
      `M${x()(0)} ${y(0)}` +
      props.pts.map((p, i) => `L${x()(i + 1).toFixed(1)} ${y(p.share).toFixed(1)}`).join(""),
  );
  return (
    <div ref={w.ref} class="px-1">
      <Show when={w.width() > 0}>
        <svg width={w.width()} height={H} role="img" aria-label="Share of the reduction from the largest changes">
          <For each={[0, 0.25, 0.5, 0.75, 1]}>
            {(t) => (
              <g>
                <line x1={m.l} x2={w.width() - m.r} y1={y(t)} y2={y(t)} stroke="var(--grid)" />
                <text x={m.l - 6} y={y(t) + 3} text-anchor="end" class="fill-faint font-mono text-[10px] tnum">
                  {t * 100}%
                </text>
              </g>
            )}
          </For>
          <For each={x().ticks(Math.max(3, Math.floor(w.width() / 90))).filter((t) => Number.isInteger(t))}>
            {(t) => (
              <text x={x()(t)} y={H - 6} text-anchor="middle" class="fill-faint font-mono text-[10px] tnum">
                {t}
              </text>
            )}
          </For>
          <line
            x1={x()(0)}
            y1={y(0)}
            x2={x()(n())}
            y2={y(1)}
            stroke="var(--border-strong)"
            stroke-dasharray="3 3"
          />
          <path d={path()} fill="none" stroke="var(--foreground)" stroke-width="1.75" stroke-linejoin="round" />
          <For each={props.marks}>
            {(k) => {
              const p = () => props.pts[k - 1];
              return (
                <g>
                  <circle cx={x()(k)} cy={y(p().share)} r="3.5" fill="var(--foreground)" stroke="var(--card)" stroke-width="2" />
                  <text
                    x={x()(k) + 6}
                    y={y(p().share) + 12}
                    class="fill-muted-foreground font-mono text-[10px] tnum"
                  >
                    {k}: {pct(p().share * 100, 0)}
                  </text>
                </g>
              );
            }}
          </For>
        </svg>
      </Show>
    </div>
  );
}

/** Tokens saved by each change in merge order, on a log scale; the selected technique stands out. */
function Sequence(props: { changes: Change[]; technique: string }) {
  const w = useWidth();
  const H = 210;
  const m = { t: 10, r: 10, b: 22, l: 46 };
  const max = () => Math.max(10, ...props.changes.map((c) => c.saved));
  const x = createMemo(() =>
    scaleLinear()
      .domain([0, Math.max(1, props.changes.length)])
      .range([m.l, Math.max(m.l + 10, w.width() - m.r)]),
  );
  const y = createMemo(() => scaleLog().domain([1, max()]).nice().range([H - m.b, m.t]).clamp(true));
  const on = (c: Change) => !props.technique || c.technique === props.technique;
  const bw = () => x()(1) - x()(0);
  return (
    <div ref={w.ref} class="px-1">
      <Show when={w.width() > 0}>
        <svg width={w.width()} height={H} role="img" aria-label="Tokens saved per change, in merge order">
          <For each={y().ticks(4).filter((t) => [1, 10, 100, 1000, 10000, 100000].includes(t))}>
            {(t) => (
              <g>
                <line x1={m.l} x2={w.width() - m.r} y1={y()(t)} y2={y()(t)} stroke="var(--grid)" />
                <text x={m.l - 6} y={y()(t) + 3} text-anchor="end" class="fill-faint font-mono text-[10px] tnum">
                  {num(t)}
                </text>
              </g>
            )}
          </For>
          <For each={x().ticks(Math.max(3, Math.floor(w.width() / 90))).filter((t) => Number.isInteger(t) && t > 0)}>
            {(t) => (
              <text x={x()(t - 0.5)} y={H - 6} text-anchor="middle" class="fill-faint font-mono text-[10px] tnum">
                {t}
              </text>
            )}
          </For>
          <For each={props.changes}>
            {(c, i) => {
              const cx = () => x()(i() + 0.5);
              return (
                <g class="cursor-pointer" onClick={() => (location.hash = `#/attempts/${c.pr}`)}>
                  <title>{`#${c.pr} ${c.title}: ${num(c.saved)} tokens`}</title>
                  <line
                    x1={cx()}
                    x2={cx()}
                    y1={H - m.b}
                    y2={y()(Math.max(1, c.saved))}
                    stroke={on(c) ? "var(--foreground)" : "var(--border)"}
                    stroke-width={Math.max(1, Math.min(3, bw() * 0.45))}
                  />
                  <Show when={on(c)}>
                    <circle cx={cx()} cy={y()(Math.max(1, c.saved))} r="2.5" fill="var(--foreground)" />
                  </Show>
                </g>
              );
            }}
          </For>
          <line x1={m.l} x2={w.width() - m.r} y1={H - m.b} y2={H - m.b} stroke="var(--border-strong)" />
        </svg>
      </Show>
    </div>
  );
}

function Bars(props: { rows: { label: string; n: number; saved: number; share: number }[]; mono?: boolean }) {
  return (
    <div class="divide-y divide-border">
      <For each={props.rows}>
        {(r) => (
          <div class="px-5 py-3">
            <div class="flex items-baseline justify-between gap-3 text-xs">
              <span class="truncate font-medium" classList={{ "font-mono": props.mono }}>
                {r.label}
              </span>
              <span class="shrink-0 font-mono text-muted-foreground tnum">
                {num(r.saved)} · {num(r.n)} {r.n === 1 ? "change" : "changes"} · {pct(r.share * 100, 0)}
              </span>
            </div>
            <ShareBar share={r.share} class="mt-2" />
          </div>
        )}
      </For>
    </div>
  );
}

function Body() {
  const techniques = () => (store.rules()?.techniques ?? []).map((t) => t.key);
  const y = createMemo(() => yieldOf(store.all(), techniques()));
  const [technique, setTechnique] = createSignal("");
  const [showAll, setShowAll] = createSignal(false);
  const describe = (k: string) =>
    k === OTHER ? "Titles that do not follow technique(scope): summary." : store.rules()?.techniques?.find((t) => t.key === k)?.description;

  const topN = () => Math.min(10, y().changes.length);
  const topShare = () => y().concentration[topN() - 1]?.share ?? 0;
  const marks = () => [1, 5, 10, 20].filter((k) => k <= y().changes.length);
  const largest = () => y().concentration.slice(0, showAll() ? 25 : 10);
  const failed = () => y().failures.reduce((s, f) => s + f.n, 0);
  const seqTechniques = createMemo(() => y().byTechnique.map((g) => g.key));
  const filtered = createMemo(() => y().changes.filter((c) => !technique() || c.technique === technique()));
  const selPeriods = createMemo(() => {
    const cs = y().changes;
    return y().periods.map((p) => {
      const xs = cs.slice(p.from, p.to).filter((c) => !technique() || c.technique === technique());
      return { ...p, n: xs.length, saved: xs.reduce((s, c) => s + c.saved, 0) };
    });
  });

  return (
    <Show
      when={y().changes.length}
      fallback={
        <div class="rounded-lg border border-dashed border-border px-6 py-16 text-center text-sm text-muted-foreground">
          Nothing has been merged yet. Once changes land, this page shows which kinds of change returned the most.
          <Show when={failed()}>
            <p class="mt-3 text-xs">
              {num(failed())} not merged: {y().failures.map((f) => `${f.label} ${f.n}`).join(" · ")}
            </p>
          </Show>
        </div>
      }
    >
      <div class="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border md:grid-cols-4">
        <Stat label="Tokens saved" value={num(y().total)} sub={`by ${num(y().changes.length)} merged changes`} />
        <Stat
          label={`Largest ${topN()} changes`}
          value={pct(topShare() * 100, 0)}
          sub="of the reduction"
        />
        <Stat
          label="Median change"
          value={num(median(y().changes.map((c) => c.saved)))}
          sub={`mean ${num(y().total / y().changes.length)}`}
        />
        <Stat
          label="Leading technique"
          value={y().byTechnique[0].key}
          sub={`${pct(y().byTechnique[0].share * 100, 0)} from ${num(y().byTechnique[0].n)} changes`}
        />
      </div>

      <Card class="mt-6">
        <CardHeader>
          <CardTitle>By technique</CardTitle>
          <span class="text-xs text-faint">tokens saved, largest first</span>
        </CardHeader>
        <TechniqueTable groups={y().byTechnique} total={y().total} describe={describe} />
        <div class="border-t border-border px-5 py-3 text-xs text-muted-foreground">
          <Show when={failed()} fallback="Every decided attempt was merged.">
            <span class="text-foreground">{num(failed())} not merged</span>, by the gate that failed:{" "}
            <span class="tnum">{y().failures.map((f) => `${f.label} ${f.n}`).join(" · ")}</span>
          </Show>
        </div>
      </Card>

      <div class="mt-6 grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Concentration</CardTitle>
            <span class="text-xs text-faint">share of the total from the largest N</span>
          </CardHeader>
          <div class="px-5 pt-4 text-sm">
            The largest {topN()} {topN() === 1 ? "change is" : "changes are"}{" "}
            <span class="font-mono font-medium tnum">{pct(topShare() * 100, 0)}</span> of the reduction of{" "}
            <span class="font-mono tnum">{num(y().total)}</span> tokens.
          </div>
          <div class="pt-2 pb-2">
            <Curve pts={y().concentration} marks={marks()} />
          </div>
          <div class="border-t border-border px-5 py-2.5 text-xs text-faint">
            Changes ranked by size along the bottom; the dashed line is what equal changes would give.
          </div>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>By size class</CardTitle>
            <span class="text-xs text-faint">tokens saved per change</span>
          </CardHeader>
          <Bars rows={y().sizes} />
          <div class="border-t border-border px-5 py-2.5 text-xs text-faint">
            Each change costs one review and one full build whatever its size.
            <Show when={store.rules()?.settings?.["score.min_delta"] != null}>
              {" "}The current minimum is {num(Number(store.rules()!.settings["score.min_delta"]))} tokens.
            </Show>
          </div>
        </Card>
      </div>

      <Card class="mt-6">
        <CardHeader class="flex-wrap">
          <CardTitle>Largest changes</CardTitle>
          <button
            type="button"
            class="text-xs text-muted-foreground hover:text-foreground"
            onClick={() => setShowAll(!showAll())}
          >
            <Show when={y().changes.length > 10}>{showAll() ? "Show 10" : `Show ${Math.min(25, y().changes.length)}`}</Show>
          </button>
        </CardHeader>
        <div class="overflow-x-auto">
          <table class="w-full text-xs">
            <thead class="text-faint">
              <tr class="border-b border-border">
                <Th left>Change</Th>
                <Th>Tokens</Th>
                <Th class="hidden pr-5 sm:table-cell">Running share</Th>
              </tr>
            </thead>
            <tbody class="divide-y divide-border">
              <For each={largest()}>
                {(r) => (
                  <tr class="hover:bg-subtle">
                    <td class="max-w-[1px] px-5 py-2.5 text-left">
                      <div class="flex gap-2">
                        <a
                          href={r.change.url}
                          target="_blank"
                          rel="noreferrer"
                          class="shrink-0 font-mono text-muted-foreground hover:underline"
                        >
                          #{r.change.pr}
                        </a>
                        <a href={`#/attempts/${r.change.pr}`} class="truncate hover:underline">
                          {r.change.title}
                        </a>
                      </div>
                    </td>
                    <Td>{num(r.change.saved)}</Td>
                    <Td muted class="hidden pr-5 sm:table-cell">
                      {pct(r.share * 100, 0)}
                    </Td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Card>

      <Card class="mt-6">
        <CardHeader class="flex-wrap">
          <CardTitle>Over time</CardTitle>
          <select
            aria-label="Highlight a technique"
            class="h-8 rounded-md border border-border bg-background px-2 text-sm text-foreground"
            value={technique()}
            onChange={(e) => setTechnique(e.currentTarget.value)}
          >
            <option value="">All techniques</option>
            <For each={seqTechniques()}>{(t) => <option value={t}>{t}</option>}</For>
          </select>
        </CardHeader>
        <div class="px-5 pt-3 text-xs text-faint">
          Tokens saved by each change, in the order merged, on a log scale.
          <Show when={technique()}> {num(filtered().length)} {technique()} changes highlighted.</Show>
        </div>
        <div class="pt-2 pb-1">
          <Sequence changes={y().changes} technique={technique()} />
        </div>
        <div class="overflow-x-auto border-t border-border">
          <table class="w-full text-xs">
            <thead class="text-faint">
              <tr class="border-b border-border">
                <Th left>Merges</Th>
                <Th>Changes</Th>
                <Th>Tokens saved</Th>
                <Th>Median</Th>
                <Th class="pr-5">Largest</Th>
              </tr>
            </thead>
            <tbody class="divide-y divide-border">
              <For each={selPeriods()}>
                {(p) => (
                  <tr>
                    <td class="px-5 py-2.5 text-left font-mono text-muted-foreground tnum">
                      {p.label}
                      <span class="ml-2 hidden font-sans text-faint sm:inline">
                        to {dateTime(y().changes[p.to - 1].at)}
                      </span>
                    </td>
                    <Td muted>{num(p.n)}</Td>
                    <Td>{num(p.saved)}</Td>
                    <Td muted>{num(p.median)}</Td>
                    <Td muted class="pr-5">
                      {num(p.largest)}
                    </Td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
          <div class="border-t border-border px-5 py-2.5 text-xs text-faint">
            Median and largest are over all techniques.
          </div>
        </div>
      </Card>

      <Card class="mt-6">
        <CardHeader>
          <CardTitle>By scope</CardTitle>
          <span class="text-xs text-faint">top-level folder; * for cross-cutting</span>
        </CardHeader>
        <Bars mono rows={y().byArea.map((g) => ({ label: g.key, n: g.n, saved: g.saved, share: g.share }))} />
      </Card>
    </Show>
  );
}

export default function Yield() {
  return (
    <Page
      title="Yield"
      subtitle="Where the reduction comes from. Every change costs one review and one full build whatever its size; this is what each kind of change returned."
    >
      <WithState>{() => <Body />}</WithState>
    </Page>
  );
}
