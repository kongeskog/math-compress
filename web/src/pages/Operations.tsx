import { useNavigate } from "@solidjs/router";
import { scaleLinear, scaleTime } from "d3-scale";
import { createMemo, createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Empty, Page } from "~/components/Page";
import { Card, CardHeader, CardTitle } from "~/components/ui/card";
import { Segmented } from "~/components/ui/segmented";
import { store } from "~/lib/data";
import { num, pct, signed } from "~/lib/format";
import type { Ops, OpsPr, OpsRun } from "~/lib/types";

const MIN = 60_000;
const HOUR = 60 * MIN;

const ms = (iso: string | null | undefined) => (iso ? new Date(iso).getTime() : undefined);

function span(d: number | undefined) {
  if (d == null || !isFinite(d)) return "—";
  const s = Math.max(0, Math.round(d / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

const clock = new Intl.DateTimeFormat("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
const dayClock = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

function quantile(xs: number[], q: number) {
  if (!xs.length) return undefined;
  const s = [...xs].sort((a, b) => a - b);
  const i = (s.length - 1) * q;
  const lo = Math.floor(i);
  const hi = Math.ceil(i);
  return s[lo] + (s[hi] - s[lo]) * (i - lo);
}

/** One change's path through the pipeline. */
type Flow = {
  p: OpsPr;
  t0: number;
  /** when the approval that counted was given */
  approved?: number;
  /** merged, closed, or the end of the window while still open */
  t1: number;
  review?: number;
  line?: number;
  lead?: number;
  sentBack: number;
  runs: OpsRun[];
};

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

/** A count over time as steps: +1 at each start, −1 at each end. */
function steps(intervals: [number, number | undefined][], from: number, to: number) {
  const ev: [number, number][] = [];
  for (const [a, b] of intervals) {
    if (a > to) continue;
    ev.push([a, 1]);
    if (b != null && b <= to) ev.push([b, -1]);
  }
  ev.sort((x, y) => x[0] - y[0] || x[1] - y[1]);
  let v = 0;
  const pts: { t: number; v: number }[] = [];
  for (const [t, d] of ev) {
    v += d;
    if (t < from) continue;
    if (!pts.length) pts.push({ t: from, v: v - d });
    pts.push({ t, v });
  }
  if (!pts.length) pts.push({ t: from, v });
  pts.push({ t: to, v: pts[pts.length - 1].v });
  return pts;
}

const valueAt = (pts: { t: number; v: number }[], t: number) => {
  let v = pts[0]?.v ?? 0;
  for (const p of pts) {
    if (p.t > t) break;
    v = p.v;
  }
  return v;
};

/**
 * Time in which the pipeline was working (a change open or a build running), and the part of it
 * in which no idea was waiting. Idle stretches, such as a night nobody worked, count for neither.
 */
function activity(
  ready: { t: number; v: number }[],
  open: { t: number; v: number }[],
  building: { t: number; v: number }[],
) {
  const ts = [...new Set([...ready, ...open, ...building].map((p) => p.t))].sort((x, y) => x - y);
  let active = 0;
  let starved = 0;
  for (let i = 0; i + 1 < ts.length; i++) {
    const d = ts[i + 1] - ts[i];
    if (valueAt(open, ts[i]) <= 0 && valueAt(building, ts[i]) <= 0) continue;
    active += d;
    if (valueAt(ready, ts[i]) <= 0) starved += d;
  }
  return { active, starved };
}

function Tip(props: { x: number; y: number; width: number; children: JSX.Element }) {
  const flip = () => props.x > props.width - 260;
  return (
    <div
      class="pointer-events-none absolute z-10 w-[240px] rounded-md border border-border bg-card p-3 text-xs shadow-lg shadow-black/5"
      style={{
        left: flip() ? undefined : `${props.x + 14}px`,
        right: flip() ? `${props.width - props.x + 14}px` : undefined,
        top: `${Math.max(props.y - 20, 4)}px`,
      }}
    >
      {props.children}
    </div>
  );
}

function TipRow(props: { label: string; value: string; swatch?: string }) {
  return (
    <div class="flex items-center justify-between gap-3 py-0.5">
      <span class="flex items-center gap-1.5 text-muted-foreground">
        <Show when={props.swatch}>
          <span class="inline-block size-2 rounded-[2px]" style={{ background: props.swatch }} />
        </Show>
        {props.label}
      </span>
      <span class="font-mono tnum">{props.value}</span>
    </div>
  );
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

function Swatch(props: { color?: string; class?: string; label: string }) {
  return (
    <span class="inline-flex items-center gap-1.5">
      <span class={`inline-block h-2 w-3 rounded-[2px] ${props.class ?? ""}`} style={{ background: props.color }} />
      {props.label}
    </span>
  );
}

const RUN_FILL: Record<string, string> = {
  success: "var(--muted-foreground)",
  failure: "var(--bad)",
  cancelled: "var(--border-strong)",
};
const runFill = (r: OpsRun) => RUN_FILL[r.outcome] ?? "var(--warn)";

/** Every change as a bar from opened to merged, split at the approval, with its verification runs underneath. */
function Timeline(props: { flows: Flow[]; mainRuns: OpsRun[]; from: number; to: number }) {
  const navigate = useNavigate();
  const w = useWidth();
  const [hover, setHover] = createSignal<{ f: Flow; y: number; x: number }>();
  const ROW = 20;
  const m = () => ({ t: 26, r: w.width() < 560 ? 8 : 64, b: 6, l: 44 });
  const height = () => m().t + (props.flows.length + 1) * ROW + m().b;
  const x = createMemo(() =>
    scaleTime()
      .domain([props.from, props.to])
      .range([m().l, Math.max(m().l + 10, w.width() - m().r)]),
  );
  const px = (t: number) => Math.round(x()(Math.min(Math.max(t, props.from), props.to)) * 2) / 2;
  const rowY = (i: number) => m().t + (i + 1) * ROW;
  const bar = (a: number, b: number, y: number, fill: string) => {
    const x0 = px(a);
    const x1 = px(b);
    return <rect x={x0} y={y} width={Math.max(x1 - x0, 2)} height="8" rx="2.5" fill={fill} />;
  };

  return (
    <div ref={w.ref} class="relative" onMouseLeave={() => setHover(undefined)}>
      <Show when={w.width() > 0}>
        <svg width={w.width()} height={height()} role="img" aria-label="Timeline of every change, from opened to merged">
          <For each={x().ticks(Math.max(3, Math.floor(w.width() / 130)))}>
            {(t) => (
              <g>
                <line x1={x()(t)} x2={x()(t)} y1={m().t - 4} y2={height() - m().b} stroke="var(--grid)" />
                <text x={x()(t)} y={12} text-anchor="middle" class="fill-faint font-mono text-[10px]">
                  {clock.format(t)}
                </text>
              </g>
            )}
          </For>

          <text x={m().l - 8} y={m().t + 11} text-anchor="end" class="fill-faint font-mono text-[10px]">
            main
          </text>
          <For each={props.mainRuns}>
            {(r) => (
              <rect
                x={px(ms(r.started_at)!)}
                y={m().t + 5}
                width={Math.max(px(ms(r.ended_at) ?? props.to) - px(ms(r.started_at)!) - 1, 1.5)}
                height="4"
                rx="1"
                fill={runFill(r)}
              />
            )}
          </For>

          <For each={props.flows}>
            {(f, i) => {
              const y = () => rowY(i());
              const split = () => (f.approved != null ? Math.min(Math.max(f.approved, f.t0), f.t1) : undefined);
              return (
                <g
                  class="cursor-pointer"
                  onMouseMove={(e) => {
                    const box = e.currentTarget.ownerSVGElement!.getBoundingClientRect();
                    setHover({ f, y: y(), x: e.clientX - box.left });
                  }}
                  onClick={() => navigate(`/attempts/${f.p.pr}`)}
                >
                  <rect
                    x="0"
                    y={y()}
                    width={w.width()}
                    height={ROW}
                    fill={hover()?.f === f ? "var(--muted)" : "transparent"}
                  />
                  <text x={m().l - 8} y={y() + 11} text-anchor="end" class="fill-muted-foreground font-mono text-[10px]">
                    #{f.p.pr}
                  </text>
                  <Show
                    when={split() != null}
                    fallback={bar(f.t0, f.t1, y() + 3, "var(--series-1)")}
                  >
                    <Show when={px(split()!) - px(f.t0) >= 3}>{bar(f.t0, split()! - 0, y() + 3, "var(--series-1)")}</Show>
                    <rect
                      x={px(split()!) + (px(split()!) - px(f.t0) >= 3 ? 2 : 0)}
                      y={y() + 3}
                      width={Math.max(px(f.t1) - px(split()!) - (px(split()!) - px(f.t0) >= 3 ? 2 : 0), 2)}
                      height="8"
                      rx="2.5"
                      fill="var(--series-2)"
                      opacity={f.p.status === "closed" ? 0.35 : 1}
                    />
                  </Show>
                  <For each={f.runs}>
                    {(r) => (
                      <rect
                        x={px(ms(r.started_at)!)}
                        y={y() + 13}
                        width={Math.max(px(ms(r.ended_at) ?? props.to) - px(ms(r.started_at)!) - 1, 1.5)}
                        height="3"
                        rx="1"
                        fill={runFill(r)}
                      />
                    )}
                  </For>
                  <For each={f.p.verdicts.filter((v) => !v.approve)}>
                    {(v) => (
                      <path
                        d={`M${px(ms(v.at)!)} ${y() + 1.5} l4.5 5.5 l-4.5 5.5 l-4.5 -5.5 z`}
                        fill="var(--foreground)"
                        stroke="var(--card)"
                        stroke-width="1.5"
                      />
                    )}
                  </For>
                  <Show when={w.width() >= 560 && f.p.status === "merged" && f.p.delta != null}>
                    <text
                      x={w.width() - 4}
                      y={y() + 11}
                      text-anchor="end"
                      class="fill-muted-foreground font-mono text-[10px] tnum"
                    >
                      {signed(f.p.delta)}
                    </text>
                  </Show>
                </g>
              );
            }}
          </For>
        </svg>
      </Show>
      <Show when={hover()}>
        {(h) => (
          <Tip x={h().x} y={h().y} width={w.width()}>
            <div class="mb-1.5 font-medium text-foreground">
              #{h().f.p.pr} <span class="font-normal text-muted-foreground">· {h().f.p.status}</span>
            </div>
            <div class="mb-2 line-clamp-2 text-muted-foreground">{h().f.p.title}</div>
            <TipRow label="Opened" value={dayClock.format(h().f.t0)} />
            <Show
              when={store.reviewOn() || h().f.approved != null}
              fallback={<TipRow swatch="var(--series-1)" label="Opened to merged" value={span(h().f.lead)} />}
            >
              <TipRow swatch="var(--series-1)" label="Until approved" value={span(h().f.review)} />
              <TipRow swatch="var(--series-2)" label="Approved to merged" value={span(h().f.line)} />
            </Show>
            <TipRow
              label="Verification runs"
              value={`${h().f.runs.length}${
                h().f.runs.some((r) => r.outcome === "cancelled")
                  ? ` (${h().f.runs.filter((r) => r.outcome === "cancelled").length} cancelled)`
                  : ""
              }`}
            />
            <Show when={h().f.sentBack}>
              <TipRow
                label={store.reviewOn() ? "Sent back by review" : "Changes requested"}
                value={`${h().f.sentBack}×`}
              />
            </Show>
            <Show when={h().f.p.delta != null}>
              <TipRow label="Tokens" value={signed(h().f.p.delta)} />
            </Show>
          </Tip>
        )}
      </Show>
    </div>
  );
}

/** One count over time, as a step line; the crosshair is shared between the charts stacked on a page. */
function StepChart(props: {
  title: string;
  note: string;
  pts: { t: number; v: number }[];
  from: number;
  to: number;
  at: number | undefined;
  onAt: (t: number | undefined) => void;
}) {
  const w = useWidth();
  const H = 96;
  const m = { t: 8, r: 10, b: 18, l: 26 };
  const max = () => Math.max(1, ...props.pts.map((p) => p.v));
  const x = createMemo(() => scaleTime().domain([props.from, props.to]).range([m.l, Math.max(m.l + 10, w.width() - m.r)]));
  const y = createMemo(() => scaleLinear().domain([0, max()]).nice(2).range([H - m.b, m.t]));
  const path = createMemo(() => {
    let d = "";
    let prev: number | undefined;
    for (const p of props.pts) {
      const X = x()(p.t).toFixed(1);
      if (prev == null) d += `M${X} ${y()(p.v).toFixed(1)}`;
      else d += `H${X}V${y()(p.v).toFixed(1)}`;
      prev = p.v;
    }
    return d;
  });
  const area = () => `${path()}V${y()(0)}H${x()(props.from).toFixed(1)}Z`;
  const now = () => valueAt(props.pts, props.to);
  return (
    <div class="px-5 py-4">
      <div class="flex items-baseline justify-between gap-3">
        <div class="text-xs font-medium">{props.title}</div>
        <div class="font-mono text-xs text-muted-foreground tnum">
          {props.at != null ? `${valueAt(props.pts, props.at)} at ${clock.format(props.at)}` : `${now()} now`}
        </div>
      </div>
      <div class="mt-0.5 text-xs text-faint">{props.note}</div>
      <div
        ref={w.ref}
        class="relative mt-2"
        onMouseMove={(e) => {
          const box = e.currentTarget.getBoundingClientRect();
          const t = x().invert(e.clientX - box.left).getTime();
          props.onAt(t >= props.from && t <= props.to ? t : undefined);
        }}
        onMouseLeave={() => props.onAt(undefined)}
      >
        <Show when={w.width() > 0}>
          <svg width={w.width()} height={H} role="img" aria-label={props.title}>
            <For each={y().ticks(2)}>
              {(t) => (
                <g>
                  <line x1={m.l} x2={w.width() - m.r} y1={y()(t)} y2={y()(t)} stroke="var(--grid)" />
                  <text x={m.l - 6} y={y()(t) + 3} text-anchor="end" class="fill-faint font-mono text-[10px] tnum">
                    {t}
                  </text>
                </g>
              )}
            </For>
            <For each={x().ticks(Math.max(3, Math.floor(w.width() / 150)))}>
              {(t) => (
                <text x={x()(t)} y={H - 4} text-anchor="middle" class="fill-faint font-mono text-[10px]">
                  {clock.format(t)}
                </text>
              )}
            </For>
            <path d={area()} fill="var(--foreground)" opacity="0.06" />
            <path d={path()} fill="none" stroke="var(--foreground)" stroke-width="1.75" stroke-linejoin="round" />
            <Show when={props.at != null}>
              <line
                x1={x()(props.at!)}
                x2={x()(props.at!)}
                y1={m.t}
                y2={H - m.b}
                stroke="var(--border-strong)"
                stroke-dasharray="3 3"
              />
              <circle
                cx={x()(props.at!)}
                cy={y()(valueAt(props.pts, props.at!))}
                r="4"
                fill="var(--foreground)"
                stroke="var(--card)"
                stroke-width="2"
              />
            </Show>
          </svg>
        </Show>
      </div>
    </div>
  );
}

/** Tokens removed per time bucket. */
function Throughput(props: { flows: Flow[]; from: number; to: number }) {
  const w = useWidth();
  const [hover, setHover] = createSignal<number>();
  const H = 190;
  const m = { t: 14, r: 10, b: 20, l: 44 };
  const size = () => {
    const range = props.to - props.from;
    return range <= 3 * HOUR ? 10 * MIN : range <= 12 * HOUR ? 30 * MIN : range <= 36 * HOUR ? HOUR : 3 * HOUR;
  };
  const buckets = createMemo(() => {
    const s = size();
    const start = Math.floor(props.from / s) * s;
    const n = Math.max(1, Math.ceil((props.to - start) / s));
    const out = Array.from({ length: n }, (_, i) => ({ t: start + i * s, saved: 0, merges: 0 }));
    for (const f of props.flows) {
      if (f.p.status !== "merged") continue;
      const b = out[Math.floor((f.t1 - start) / s)];
      if (!b) continue;
      b.saved += -(f.p.delta ?? 0);
      b.merges += 1;
    }
    return out;
  });
  const x = createMemo(() =>
    scaleLinear()
      .domain([0, buckets().length])
      .range([m.l, Math.max(m.l + 10, w.width() - m.r)]),
  );
  const y = createMemo(() =>
    scaleLinear()
      .domain([0, Math.max(1, ...buckets().map((b) => b.saved))])
      .nice(3)
      .range([H - m.b, m.t]),
  );
  const bw = () => x()(1) - x()(0);
  const labelEvery = () => Math.max(1, Math.ceil(70 / Math.max(bw(), 1)));
  return (
    <div ref={w.ref} class="relative px-1 pt-2 pb-1" onMouseLeave={() => setHover(undefined)}>
      <Show when={w.width() > 0}>
        <svg width={w.width()} height={H} role="img" aria-label="Tokens removed per period">
          <For each={y().ticks(3)}>
            {(t) => (
              <g>
                <line x1={m.l} x2={w.width() - m.r} y1={y()(t)} y2={y()(t)} stroke="var(--grid)" />
                <text x={m.l - 6} y={y()(t) + 3} text-anchor="end" class="fill-faint font-mono text-[10px] tnum">
                  {num(t)}
                </text>
              </g>
            )}
          </For>
          <For each={buckets()}>
            {(b, i) => {
              const g = () => Math.min(2, bw() / 4);
              const x0 = () => x()(i()) + g() / 2;
              const bwid = () => Math.max(bw() - g(), 1);
              const top = () => y()(b.saved);
              const r = () => Math.min(3, bwid() / 2, (H - m.b - top()) / 2);
              return (
                <g onMouseEnter={() => setHover(i())}>
                  <rect x={x()(i())} y={m.t} width={bw()} height={H - m.b - m.t} fill="transparent" />
                  <Show when={b.saved > 0}>
                    <path
                      d={`M${x0()} ${H - m.b}V${top() + r()}q0 ${-r()} ${r()} ${-r()}h${bwid() - 2 * r()}q${r()} 0 ${r()} ${r()}V${H - m.b}Z`}
                      fill="var(--foreground)"
                      opacity={hover() == null || hover() === i() ? 1 : 0.35}
                    />
                  </Show>
                  <Show when={i() % labelEvery() === 0}>
                    <text x={x()(i())} y={H - 5} text-anchor="middle" class="fill-faint font-mono text-[10px]">
                      {clock.format(b.t)}
                    </text>
                  </Show>
                </g>
              );
            }}
          </For>
          <line x1={m.l} x2={w.width() - m.r} y1={H - m.b} y2={H - m.b} stroke="var(--border-strong)" />
        </svg>
      </Show>
      <Show when={hover() != null && buckets()[hover()!]}>
        {(_) => {
          const b = () => buckets()[hover()!];
          return (
            <Tip x={x()(hover()! + 0.5)} y={Math.max(y()(b().saved), 30)} width={w.width()}>
              <div class="mb-1.5 font-medium">
                {clock.format(b().t)}–{clock.format(b().t + size())}
              </div>
              <TipRow label="Tokens removed" value={num(b().saved)} />
              <TipRow label="Changes merged" value={String(b().merges)} />
            </Tip>
          );
        }}
      </Show>
    </div>
  );
}

type Range = "2" | "6" | "24" | "all";

function Body(props: { ops: Ops; now: number }) {
  // Open on the current burst of work when there is one, otherwise on everything.
  const recent = (h: number) => props.ops.prs.filter((p) => (ms(p.closed_at) ?? props.now) >= props.now - h * HOUR).length;
  const [range, setRange] = createSignal<Range>(recent(6) >= 5 ? "6" : recent(24) >= 5 ? "24" : "all");
  const [at, setAt] = createSignal<number>();

  const to = () => props.now;
  const first = createMemo(() => {
    const ts = [
      ...props.ops.prs.map((p) => ms(p.created_at)!),
      ...props.ops.ideas.map((i) => ms(i.created_at)!),
      ...props.ops.runs.map((r) => ms(r.created_at)!),
    ];
    return ts.length ? Math.min(...ts) : to() - HOUR;
  });
  const from = () => (range() === "all" ? first() : Math.max(first(), to() - Number(range()) * HOUR));

  const runsByPr = createMemo(() => {
    const by = new Map<number, OpsRun[]>();
    for (const r of props.ops.runs) by.set(r.pr, [...(by.get(r.pr) ?? []), r]);
    return by;
  });

  /** Changes that overlap the period. */
  const flows = createMemo<Flow[]>(() =>
    props.ops.prs
      .map((p) => {
        const t0 = ms(p.created_at)!;
        const closed = ms(p.closed_at);
        const last = p.verdicts[p.verdicts.length - 1];
        const approved = last?.approve
          ? ms([...p.verdicts].reverse().find((v) => v.approve)!.at)
          : undefined;
        const t1 = closed ?? to();
        const merged = p.status === "merged";
        const split = approved != null ? Math.min(Math.max(approved, t0), t1) : undefined;
        // Where no change needs an approval, the whole time from opened to
        // merged is verification and the merge line.
        const whole = !store.reviewOn() && split == null && merged ? t1 - t0 : undefined;
        return {
          p,
          t0,
          approved,
          t1,
          review: split != null ? split - t0 : undefined,
          line: split != null && closed != null ? closed - split : whole,
          lead: merged ? t1 - t0 : undefined,
          sentBack: p.verdicts.filter((v) => !v.approve).length,
          runs: (runsByPr().get(p.pr) ?? []).filter((r) => (ms(r.ended_at) ?? to()) >= from()),
        };
      })
      .filter((f) => f.t1 >= from() && f.t0 <= to())
      .sort((a, b) => a.t0 - b.t0),
  );
  const merged = createMemo(() => flows().filter((f) => f.p.status === "merged" && f.t1 >= from()));
  const runs = createMemo(() => props.ops.runs.filter((r) => (ms(r.ended_at) ?? to()) >= from()));

  const med = (pick: (f: Flow) => number | undefined, q = 0.5) =>
    quantile(
      merged()
        .map(pick)
        .filter((v): v is number => v != null),
      q,
    );
  const minutes = (rs: OpsRun[]) =>
    rs.reduce((s, r) => s + ((ms(r.ended_at) ?? to()) - ms(r.started_at)!), 0) / MIN;
  const saved = () => merged().reduce((s, f) => s - (f.p.delta ?? 0), 0);

  const series = createMemo(() => ({
    ready: steps(
      props.ops.ideas.map((i) => {
        const taken = [ms(i.claimed_at), ms(i.pr_at), ms(i.closed_at)].filter((t): t is number => t != null);
        return [ms(i.created_at)!, taken.length ? Math.min(...taken) : undefined];
      }),
      from(),
      to(),
    ),
    open: steps(
      props.ops.prs.map((p) => [ms(p.created_at)!, ms(p.closed_at)]),
      from(),
      to(),
    ),
    building: steps(
      props.ops.runs.map((r) => [ms(r.started_at)!, ms(r.ended_at)]),
      from(),
      to(),
    ),
  }));

  const act = createMemo(() => activity(series().ready, series().open, series().building));
  /** Hours in which the pipeline was working; rates are per active hour. */
  const hours = () => Math.max(act().active / HOUR, 1 / 60);

  const kinds = createMemo(() =>
    (
      [
        ["own", "First check of a change", "started when a change is opened or updated"],
        ["line", "Merge line", "re-checks on top of the changes ahead in line"],
        ["main", "Main", "re-check of main after each merge"],
      ] as const
    ).map(([kind, label, note]) => {
      const rs = runs().filter((r) => r.kind === kind);
      const cancelled = rs.filter((r) => r.outcome === "cancelled");
      const used = rs.filter((r) => r.outcome !== "cancelled");
      return { kind, label, note, n: rs.length, nCancelled: cancelled.length, used: minutes(used), lost: minutes(cancelled) };
    }),
  );
  const maxKind = () => Math.max(1, ...kinds().map((k) => k.used + k.lost));
  const lostShare = () => {
    const lost = kinds().reduce((s, k) => s + k.lost, 0);
    const all = kinds().reduce((s, k) => s + k.used + k.lost, 0);
    return all > 0 ? lost / all : 0;
  };

  /** Where the time goes, largest share first. */
  const findings = createMemo(() => {
    const review = med((f) => f.review) ?? 0;
    const line = med((f) => f.line) ?? 0;
    const total = review + line;
    const starved = act().active > 0 ? act().starved / act().active : 0;
    const out = [
      {
        key: "line",
        label: "Verification and merge line",
        share: total ? line / total : 0,
        value: span(line),
        text: `${
          store.reviewOn()
            ? `Median time from approval to merge. It is ${pct(total ? (line / total) * 100 : 0, 0)} of a change's time in the pipeline`
            : "Median time from opening a change to its merge; no approval is waited for"
        }; one verification takes about ${span(
          quantile(
            runs()
              .filter((r) => r.outcome === "success" && r.kind !== "main")
              .map((r) => ms(r.ended_at)! - ms(r.started_at)!),
            0.5,
          ),
        )}.`,
      },
      ...(store.reviewOn()
        ? [
            {
              key: "review",
              label: "Review",
              share: total ? review / total : 0,
              value: span(review),
              text: `Median time from opening a change to the approval that counted. ${merged().filter((f) => f.sentBack).length} of ${merged().length} merged changes were sent back at least once.`,
            },
          ]
        : []),
      {
        key: "ideas",
        label: "Idea supply",
        share: starved,
        value: pct(starved * 100, 0),
        text: "Share of active time in which no idea was waiting to be picked up, so a free contributor had nothing queued.",
      },
      {
        key: "waste",
        label: "Discarded builds",
        share: lostShare(),
        value: pct(lostShare() * 100, 0),
        text: "Share of build minutes spent on verification runs that were cancelled before they reported.",
      },
    ];
    return out.sort((a, b) => b.share - a.share);
  });

  const slowest = createMemo(() => [...merged()].sort((a, b) => (b.lead ?? 0) - (a.lead ?? 0)).slice(0, 8));

  return (
    <>
      <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
        <Segmented<Range>
          label="Period"
          value={range()}
          onChange={setRange}
          options={[
            { value: "2", label: "Last 2 h" },
            { value: "6", label: "Last 6 h" },
            { value: "24", label: "Last 24 h" },
            { value: "all", label: "3 days" },
          ]}
        />
        <span class="text-xs text-faint">
          {dayClock.format(from())} – {dayClock.format(to())}
        </span>
      </div>

      <div
        class="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border"
        classList={{ "md:grid-cols-5": store.reviewOn(), "md:grid-cols-3": !store.reviewOn() }}
      >
        <Stat
          label="Opened to merged"
          value={span(med((f) => f.lead))}
          sub={`median · 90% within ${span(med((f) => f.lead, 0.9))}`}
        />
        <Show when={store.reviewOn()}>
          <Stat label="Waiting for approval" value={span(med((f) => f.review))} sub="median, opened to approved" />
          <Stat label="Approved to merged" value={span(med((f) => f.line))} sub="median, verification and line" />
        </Show>
        <Stat
          label="Builds per merge"
          value={
            merged().length
              ? (merged().reduce((s, f) => s + f.runs.length, 0) / merged().length).toFixed(1)
              : "—"
          }
          sub={`${pct(lostShare() * 100, 0)} of build minutes discarded`}
        />
        <Stat
          wide
          label="Tokens per active hour"
          value={num(saved() / hours())}
          sub={`${merged().length} merged in ${span(act().active)} of activity`}
        />
      </div>

      <Card class="mt-6">
        <CardHeader>
          <CardTitle>Where the time goes</CardTitle>
          <span class="text-xs text-faint">largest share first</span>
        </CardHeader>
        <div class="grid gap-px bg-border sm:grid-cols-2 lg:grid-cols-4">
          <For each={findings()}>
            {(f, i) => (
              <div class="bg-card px-5 py-4">
                <div class="flex items-center justify-between gap-2">
                  <span class="text-xs font-medium">{f.label}</span>
                  <Show when={i() === 0 && f.share > 0}>
                    <span class="rounded-full border border-border-strong px-2 py-0.5 text-[10px] font-medium">
                      Largest
                    </span>
                  </Show>
                </div>
                <div class="mt-2 font-mono text-xl font-medium tracking-tight tnum">{f.value}</div>
                <div class="mt-2 h-1 overflow-hidden rounded-full bg-muted">
                  <div
                    class="h-full rounded-full bg-foreground"
                    style={{ width: `${Math.min(100, Math.max(f.share * 100, f.share > 0 ? 2 : 0))}%` }}
                  />
                </div>
                <p class="mt-3 text-xs leading-relaxed text-muted-foreground">{f.text}</p>
              </div>
            )}
          </For>
        </div>
      </Card>

      <Card class="mt-6">
        <CardHeader class="flex-wrap">
          <CardTitle>Every change, from opened to merged</CardTitle>
          <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <Show
              when={store.reviewOn()}
              fallback={<Swatch color="var(--series-1)" label="Open: verifying or in line" />}
            >
              <Swatch color="var(--series-1)" label="Waiting for approval" />
              <Swatch color="var(--series-2)" label="Approved, verifying or in line" />
            </Show>
            <Swatch color="var(--muted-foreground)" class="!h-1" label="Verification run" />
            <Swatch color="var(--border-strong)" class="!h-1" label="Cancelled run" />
            <span class="inline-flex items-center gap-1.5">
              <svg width="9" height="11" viewBox="-4.5 0 9 11" aria-hidden="true">
                <path d="M0 0 l4.5 5.5 l-4.5 5.5 l-4.5 -5.5 z" fill="var(--foreground)" />
              </svg>
              {store.reviewOn() ? "Sent back by review" : "Changes requested"}
            </span>
          </div>
        </CardHeader>
        <Show when={flows().length} fallback={<Empty>No changes in this period.</Empty>}>
          <div class="px-2 pt-3 pb-2">
            <Timeline
              flows={flows()}
              mainRuns={runs().filter((r) => r.kind === "main")}
              from={from()}
              to={to()}
            />
          </div>
        </Show>
      </Card>

      <div class="mt-6 grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Queues</CardTitle>
            <span class="text-xs text-faint">count over time</span>
          </CardHeader>
          <div class="divide-y divide-border">
            <StepChart
              title="Ideas waiting to be picked up"
              note="At zero, a free contributor has nothing queued."
              pts={series().ready}
              from={from()}
              to={to()}
              at={at()}
              onAt={setAt}
            />
            <StepChart
              title="Changes open"
              note="Opened and not yet merged or closed."
              pts={series().open}
              from={from()}
              to={to()}
              at={at()}
              onAt={setAt}
            />
            <StepChart
              title="Verification runs in progress"
              note="Builds running at the same time, including re-checks of main."
              pts={series().building}
              from={from()}
              to={to()}
              at={at()}
              onAt={setAt}
            />
          </div>
        </Card>

        <div class="flex flex-col gap-6">
          <Card>
            <CardHeader>
              <CardTitle>Tokens removed</CardTitle>
              <span class="text-xs text-faint">by time of merge</span>
            </CardHeader>
            <Throughput flows={flows()} from={from()} to={to()} />
          </Card>

          <Card>
            <CardHeader class="flex-wrap">
              <CardTitle>Build minutes</CardTitle>
              <div class="flex items-center gap-4 text-xs text-muted-foreground">
                <Swatch color="var(--foreground)" label="Reported a result" />
                <Swatch class="hatch" label="Cancelled" />
              </div>
            </CardHeader>
            <div class="divide-y divide-border">
              <For each={kinds()}>
                {(k) => (
                  <div class="px-5 py-3.5">
                    <div class="flex items-baseline justify-between gap-3">
                      <span class="text-xs font-medium">{k.label}</span>
                      <span class="font-mono text-xs text-muted-foreground tnum">
                        {num(k.used + k.lost)} min · {k.n} runs
                      </span>
                    </div>
                    <div class="mt-0.5 text-xs text-faint">{k.note}</div>
                    <div class="mt-2.5 flex h-2 gap-0.5" title={`${num(k.used)} min reported, ${num(k.lost)} min cancelled`}>
                      <Show when={k.used > 0}>
                        <div
                          class="rounded-[3px] bg-foreground"
                          style={{ width: `${(k.used / maxKind()) * 100}%` }}
                        />
                      </Show>
                      <Show when={k.lost > 0}>
                        <div class="hatch rounded-[3px]" style={{ width: `${(k.lost / maxKind()) * 100}%` }} />
                      </Show>
                    </div>
                    <div class="mt-1.5 text-xs text-muted-foreground tnum">
                      {num(k.lost)} min in {k.nCancelled} cancelled runs
                      <Show when={k.used + k.lost > 0}> ({pct((k.lost / (k.used + k.lost)) * 100, 0)})</Show>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </Card>
        </div>
      </div>

      <Card class="mt-6">
        <CardHeader>
          <CardTitle>Slowest merged changes</CardTitle>
          <span class="text-xs text-faint">by time from opened to merged</span>
        </CardHeader>
        <Show when={slowest().length} fallback={<Empty>Nothing merged in this period.</Empty>}>
          <div class="overflow-x-auto">
            <table class="w-full text-left text-xs">
              <thead class="text-faint">
                <tr class="border-b border-border">
                  <th class="px-5 py-2.5 font-normal">Change</th>
                  <th class="px-3 py-2.5 text-right font-normal">Total</th>
                  <Show when={store.reviewOn()}>
                    <th class="px-3 py-2.5 text-right font-normal">To approval</th>
                    <th class="px-3 py-2.5 text-right font-normal">To merge</th>
                  </Show>
                  <th class="px-3 py-2.5 text-right font-normal">Runs</th>
                  <th class="px-3 py-2.5 text-right font-normal">
                    {store.reviewOn() ? "Sent back" : "Changes requested"}
                  </th>
                  <th class="px-5 py-2.5 text-right font-normal">Tokens</th>
                </tr>
              </thead>
              <tbody class="divide-y divide-border">
                <For each={slowest()}>
                  {(f) => (
                    <tr class="hover:bg-subtle">
                      <td class="max-w-[22rem] px-5 py-2.5">
                        <a href={`#/attempts/${f.p.pr}`} class="flex gap-2 hover:underline">
                          <span class="font-mono text-muted-foreground">#{f.p.pr}</span>
                          <span class="truncate">{f.p.title}</span>
                        </a>
                      </td>
                      <td class="px-3 py-2.5 text-right font-mono tnum">{span(f.lead)}</td>
                      <Show when={store.reviewOn()}>
                        <td class="px-3 py-2.5 text-right font-mono text-muted-foreground tnum">{span(f.review)}</td>
                        <td class="px-3 py-2.5 text-right font-mono text-muted-foreground tnum">{span(f.line)}</td>
                      </Show>
                      <td class="px-3 py-2.5 text-right font-mono text-muted-foreground tnum">{f.runs.length}</td>
                      <td class="px-3 py-2.5 text-right font-mono text-muted-foreground tnum">{f.sentBack || "—"}</td>
                      <td class="px-5 py-2.5 text-right font-mono tnum">{signed(f.p.delta)}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        </Show>
      </Card>
    </>
  );
}

export default function Operations() {
  // The page reads the clock once per data refresh, not every second.
  const now = createMemo(() => store.fetchedAt() ?? Date.now());
  return (
    <Page
      title="Operations"
      subtitle={`Where time goes between an idea and a merge, measured from the pull requests${
        store.reviewOn() ? ", reviews" : ""
      } and verification runs of the last three days.`}
    >
      <Show
        when={store.ops()}
        fallback={
          <div class="rounded-lg border border-dashed border-border px-6 py-16 text-center text-sm text-muted-foreground">
            <Show when={store.fetchedAt()} fallback="Loading the pipeline timings…">
              No pipeline timings have been published yet. They appear after the next gatekeeper pass.
            </Show>
          </div>
        }
      >
        {(o) => <Body ops={o()} now={now()} />}
      </Show>
    </Page>
  );
}
