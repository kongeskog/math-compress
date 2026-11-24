import { scaleLinear, scaleTime } from "d3-scale";
import { area, curveStepAfter, line } from "d3-shape";
import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { compact, dateTime, day, metricValue, signed } from "~/lib/format";
import type { Attempt, MetricKey, Metrics } from "~/lib/types";

type Kind = "baseline" | "merged" | "rejected" | "open";
type Pt = {
  kind: Kind;
  a?: Attempt;
  i: number;
  t: number;
  /** true value, may be outside the plotted range */
  v: number;
  /** no measured value: drawn at the best known at that time */
  hollow: boolean;
};

export type ChartProps = {
  baseline: Metrics;
  baselineAt: string;
  decided: Attempt[];
  open: Attempt[];
  metric: MetricKey;
  metricLabel?: string;
  primary?: string;
  xMode?: "order" | "time";
  height?: number;
  compact?: boolean;
  onSelect?: (pr: number) => void;
};

const COLOR: Record<Kind, string> = {
  baseline: "var(--foreground)",
  merged: "var(--good)",
  rejected: "var(--faint)",
  open: "var(--warn)",
};

export function HillclimbChart(props: ChartProps) {
  let host!: HTMLDivElement;
  const [width, setWidth] = createSignal(0);
  const [hover, setHover] = createSignal<Pt>();

  onMount(() => {
    const ro = new ResizeObserver((e) => setWidth(Math.floor(e[0].contentRect.width)));
    ro.observe(host);
    onCleanup(() => ro.disconnect());
  });

  const height = () => props.height ?? 380;
  const m = () =>
    props.compact
      ? { t: 8, r: 8, b: 8, l: 8 }
      : { t: 18, r: 18, b: 30, l: width() < 520 ? 58 : 68 };

  const series = createMemo(() => {
    const key = props.metric;
    const base = props.baseline[key] ?? 0;
    const t0 = new Date(props.baselineAt).getTime();
    const pts: Pt[] = [{ kind: "baseline", i: 0, t: t0, v: base, hollow: false }];
    const best: { i: number; t: number; v: number }[] = [{ i: 0, t: t0, v: base }];
    let cur = base;
    let i = 0;
    for (const a of props.decided) {
      i += 1;
      const t = new Date(a.decided_at ?? a.created_at).getTime();
      const measured = a.metrics?.[key];
      if (a.status === "merged") {
        if (measured != null) cur = measured;
        pts.push({ kind: "merged", a, i, t, v: cur, hollow: measured == null });
        best.push({ i, t, v: cur });
      } else {
        pts.push({ kind: "rejected", a, i, t, v: measured ?? cur, hollow: measured == null });
      }
    }
    let lastT = pts[pts.length - 1].t;
    for (const a of props.open) {
      i += 1;
      const t = Math.max(new Date(a.created_at).getTime(), lastT);
      lastT = t;
      const measured = a.metrics?.[key];
      pts.push({ kind: "open", a, i, t, v: measured ?? cur, hollow: measured == null });
    }
    return { pts, best, n: Math.max(i, 1), t0, t1: Math.max(lastT, t0 + 3_600_000), cur };
  });

  const scales = createMemo(() => {
    const s = series();
    const mm = m();
    const w = Math.max(width(), 10);
    const h = height();
    const bestVals = s.best.map((b) => b.v);
    const lo = Math.min(...bestVals, ...s.pts.filter((p) => p.kind === "open").map((p) => p.v));
    const hi = Math.max(...bestVals);
    const span = hi - lo || Math.max(Math.abs(hi) * 0.005, 1);
    const yMin = lo - span * 0.14;
    const yMax = hi + span * 0.12;
    const y = scaleLinear().domain([yMin, yMax]).range([h - mm.b, mm.t]);
    const xLin = scaleLinear().domain([0, s.n]).range([mm.l, w - mm.r]);
    const xTime = scaleTime().domain([s.t0, s.t1]).range([mm.l, w - mm.r]);
    const time = props.xMode === "time";
    const px = (p: { i: number; t: number }) => (time ? xTime(p.t) : xLin(p.i));
    const py = (v: number) => y(Math.min(Math.max(v, yMin), yMax));
    return { y, xLin, xTime, px, py, yMin, yMax, w, h, mm, time };
  });

  const paths = createMemo(() => {
    const s = series();
    const sc = scales();
    const end = { i: s.n, t: s.t1, v: s.best[s.best.length - 1].v };
    const data = [...s.best, end];
    const l = line<(typeof data)[number]>()
      .x((d) => sc.px(d))
      .y((d) => sc.py(d.v))
      .curve(curveStepAfter);
    const ar = area<(typeof data)[number]>()
      .x((d) => sc.px(d))
      .y0(sc.h - sc.mm.b)
      .y1((d) => sc.py(d.v))
      .curve(curveStepAfter);
    return { line: l(data) ?? "", area: ar(data) ?? "" };
  });

  const yTicks = createMemo(() => scales().y.ticks(props.compact ? 3 : 5));
  // When the visible range is small, abbreviated labels would all read the same.
  const yTickStep = createMemo(() => {
    const t = yTicks();
    return t.length > 1 ? Math.abs(t[1] - t[0]) : Infinity;
  });
  const xTicks = createMemo(() => {
    const sc = scales();
    const count = Math.max(2, Math.min(8, Math.floor(sc.w / 110)));
    if (sc.time)
      return sc.xTime.ticks(count).map((d) => ({
        x: sc.xTime(d),
        label: d.getHours() === 0 ? day(d) : `${String(d.getHours()).padStart(2, "0")}:00`,
      }));
    return sc.xLin
      .ticks(count)
      .filter((v) => Number.isInteger(v))
      .map((v) => ({ x: sc.xLin(v), label: v === 0 ? "base" : `#${v}` }));
  });

  const fmtTick = (v: number) =>
    props.metric === "build_seconds"
      ? `${(v / 60).toFixed(1)}m`
      : yTickStep() < 1000
        ? v.toLocaleString("en-US")
        : compact(v);

  function onMove(e: PointerEvent) {
    if (props.compact) return;
    const r = host.getBoundingClientRect();
    const mx = e.clientX - r.left;
    const my = e.clientY - r.top;
    const sc = scales();
    let bestPt: Pt | undefined;
    let bestD = 28 * 28;
    for (const p of series().pts) {
      const dx = sc.px(p) - mx;
      const dy = sc.py(p.v) - my;
      const d = dx * dx + dy * dy * 0.6;
      if (d < bestD) {
        bestD = d;
        bestPt = p;
      }
    }
    setHover(bestPt);
  }

  const clamped = (p: Pt) => p.v > scales().yMax || p.v < scales().yMin;

  const summary = () => {
    const s = series();
    return `${props.metricLabel ?? props.metric}: baseline ${metricValue(props.metric, props.baseline[props.metric])}, current best ${metricValue(props.metric, s.cur)}, ${props.decided.length} decided attempts, ${props.open.length} in flight.`;
  };

  return (
    <div
      ref={host}
      class="relative w-full select-none"
      style={{ height: `${height()}px` }}
      onPointerMove={onMove}
      onPointerLeave={() => setHover(undefined)}
      onClick={() => {
        const h = hover();
        if (h?.a && props.onSelect) props.onSelect(h.a.pr);
      }}
    >
      <Show when={width() > 0}>
        <svg
          width={scales().w}
          height={scales().h}
          role="img"
          aria-label={summary()}
          class="block"
          style={{ cursor: hover()?.a && props.onSelect ? "pointer" : "default" }}
        >
          <defs>
            <linearGradient id={`hc-fill-${props.compact ? "c" : "f"}`} x1="0" x2="0" y1="0" y2="1">
              <stop offset="0%" stop-color="var(--foreground)" stop-opacity="0.07" />
              <stop offset="100%" stop-color="var(--foreground)" stop-opacity="0" />
            </linearGradient>
          </defs>

          <Show when={!props.compact}>
            <For each={yTicks()}>
              {(t) => (
                <g>
                  <line
                    x1={scales().mm.l}
                    x2={scales().w - scales().mm.r}
                    y1={scales().y(t)}
                    y2={scales().y(t)}
                    stroke="var(--grid)"
                    stroke-width="1"
                  />
                  <text
                    x={scales().mm.l - 10}
                    y={scales().y(t)}
                    dy="0.32em"
                    text-anchor="end"
                    class="fill-faint font-mono text-[11px]"
                  >
                    {fmtTick(t)}
                  </text>
                </g>
              )}
            </For>
            <For each={xTicks()}>
              {(t) => (
                <text
                  x={t.x}
                  y={scales().h - 8}
                  text-anchor="middle"
                  class="fill-faint font-mono text-[11px]"
                >
                  {t.label}
                </text>
              )}
            </For>
          </Show>

          <path d={paths().area} fill={`url(#hc-fill-${props.compact ? "c" : "f"})`} />

          {/* rejected and open first so the kept line sits on top */}
          <For each={series().pts.filter((p) => p.kind === "rejected")}>
            {(p) => (
              <circle
                cx={scales().px(p)}
                cy={scales().py(p.v)}
                r={props.compact ? 2 : 3}
                fill={p.hollow ? "var(--background)" : COLOR.rejected}
                stroke={COLOR.rejected}
                stroke-width="1.25"
                opacity={clamped(p) ? 0.45 : 0.8}
              />
            )}
          </For>

          <path
            d={paths().line}
            fill="none"
            stroke="var(--foreground)"
            stroke-width={props.compact ? 1.5 : 1.75}
            stroke-linejoin="round"
          />

          <For each={series().pts.filter((p) => p.kind === "merged" || p.kind === "baseline")}>
            {(p) => (
              <circle
                cx={scales().px(p)}
                cy={scales().py(p.v)}
                r={props.compact ? 2.5 : 4}
                fill={p.hollow ? "var(--background)" : COLOR[p.kind]}
                stroke={p.hollow ? COLOR[p.kind] : "var(--background)"}
                stroke-width="1.5"
              />
            )}
          </For>
          <For each={series().pts.filter((p) => p.kind === "open")}>
            {(p) => (
              <circle
                cx={scales().px(p)}
                cy={scales().py(p.v)}
                r={props.compact ? 2.5 : 4}
                fill={p.hollow ? "var(--background)" : COLOR.open}
                stroke={p.hollow ? COLOR.open : "var(--background)"}
                stroke-width="1.5"
              />
            )}
          </For>

          <Show when={hover()}>
            {(h) => (
              <g pointer-events="none">
                <line
                  x1={scales().px(h())}
                  x2={scales().px(h())}
                  y1={scales().mm.t}
                  y2={scales().h - scales().mm.b}
                  stroke="var(--border-strong)"
                  stroke-dasharray="3 3"
                />
                <circle
                  cx={scales().px(h())}
                  cy={scales().py(h().v)}
                  r="7"
                  fill="none"
                  stroke={COLOR[h().kind]}
                  stroke-width="1.5"
                />
              </g>
            )}
          </Show>
        </svg>

        <Show when={hover()}>
          {(h) => {
            const left = () => scales().px(h());
            const flip = () => left() > scales().w - 250;
            return (
              <div
                class="pointer-events-none absolute z-10 w-[230px] rounded-md border border-border bg-card p-3 text-xs shadow-lg shadow-black/5"
                style={{
                  left: flip() ? undefined : `${left() + 14}px`,
                  right: flip() ? `${scales().w - left() + 14}px` : undefined,
                  top: `${Math.min(Math.max(scales().py(h().v) - 40, 4), scales().h - 130)}px`,
                }}
              >
                <Show
                  when={h().a}
                  fallback={
                    <>
                      <div class="font-medium">Baseline</div>
                      <div class="mt-1 text-muted-foreground">Upstream proof before any attempt</div>
                    </>
                  }
                >
                  {(a) => (
                    <>
                      <div class="flex items-center justify-between gap-2">
                        <span class="font-mono text-muted-foreground">#{a().pr}</span>
                        <span class="flex items-center gap-1.5 capitalize" style={{ color: COLOR[h().kind] }}>
                          <span class="size-1.5 rounded-full" style={{ background: COLOR[h().kind] }} />
                          {h().kind === "open" ? a().status.replace("_", " ") : h().kind}
                        </span>
                      </div>
                      <div class="mt-1.5 line-clamp-2 font-medium leading-snug">{a().title}</div>
                      <div class="mt-1 text-muted-foreground">
                        @{a().author} · {dateTime(a().decided_at ?? a().created_at)}
                      </div>
                    </>
                  )}
                </Show>
                <div class="mt-2 flex items-baseline justify-between border-t border-border pt-2 font-mono tnum">
                  <span>{h().hollow ? "not measured" : metricValue(props.metric, h().v)}</span>
                  <Show when={h().a?.delta != null && props.metric === (props.primary ?? "tokens")}>
                    <span class={h().a!.delta! < 0 ? "text-good" : "text-muted-foreground"}>
                      {signed(h().a!.delta)}
                    </span>
                  </Show>
                </div>
              </div>
            );
          }}
        </Show>
      </Show>
    </div>
  );
}

export function ChartLegend() {
  const item = (color: string, label: string, hollow = false) => (
    <span class="inline-flex items-center gap-1.5">
      <span
        class="size-2 rounded-full border"
        style={{ background: hollow ? "transparent" : color, "border-color": color }}
      />
      {label}
    </span>
  );
  return (
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
      <span class="inline-flex items-center gap-1.5">
        <span class="h-px w-4 bg-foreground" />
        Running best
      </span>
      {item("var(--good)", "Merged")}
      {item("var(--faint)", "Rejected")}
      {item("var(--warn)", "In flight")}
      {item("var(--faint)", "Not measured", true)}
    </div>
  );
}
