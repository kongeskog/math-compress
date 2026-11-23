const nf = new Intl.NumberFormat("en-US");
const cf = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 1 });
const cf2 = new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 2 });

export const num = (n: number | null | undefined) => (n == null ? "—" : nf.format(Math.round(n)));
export const compact = (n: number | null | undefined) => (n == null ? "—" : cf.format(n));
export const compact2 = (n: number | null | undefined) => (n == null ? "—" : cf2.format(n));

/** Signed change with a true minus sign; negative is an improvement. */
export function signed(n: number | null | undefined) {
  if (n == null) return "—";
  if (n === 0) return "±0";
  return (n < 0 ? "−" : "+") + nf.format(Math.abs(Math.round(n)));
}

export function pct(n: number, digits = 2) {
  return `${n.toFixed(digits)}%`;
}

export function duration(seconds: number | null | undefined) {
  if (seconds == null) return "—";
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return m > 0 ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
}

export function metricValue(key: string, v: number | null | undefined) {
  if (v == null) return "—";
  if (key === "build_seconds") return duration(v);
  return num(v);
}

export function ago(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "—";
  const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

export function until(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "";
  const s = Math.round((new Date(iso).getTime() - now) / 1000);
  if (s <= 0) return "expired";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m left`;
  return `${Math.floor(m / 60)}h ${m % 60}m left`;
}

const df = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" });
const dtf = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});
export const day = (d: Date | string) => df.format(new Date(d));
export const dateTime = (d: Date | string | null | undefined) => (d ? dtf.format(new Date(d)) : "—");
export const sha = (s: string | null | undefined) => (s ? s.slice(0, 7) : "—");
