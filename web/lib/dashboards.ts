import { listHref } from "@/lib/list-query";
import type { RollupPoint } from "@/lib/server/entities/dashboards";
import { ChartMeasure, MetricsGroup } from "@/lib/types/enums";
import { readTagSort } from "@/lib/types/traces";

// What the dashboards page draws of the project's hourly totals: its charts,
// as a dashboard and the page's URL name them; a bar an hour or a day for
// each; and a split chart's values over time, with their totals beside it.

/** The most charts one dashboard draws, as the server takes them. */
export const DASHBOARD_CHARTS_MAX = 12;

/** The most values a split read from the spans names before the rest are
 *  summed as one, as the server reads them. */
export const ROLLUP_GROUP_TOP = 10;

/** What a spend chart may be split by: tool and status split no spend. */
export const SPEND_GROUPS = [
  MetricsGroup.Model,
  MetricsGroup.Agent,
  MetricsGroup.Provider,
  MetricsGroup.Environment,
  MetricsGroup.Release,
  MetricsGroup.Tag,
  MetricsGroup.User,
  MetricsGroup.Session,
] as const;
export type SpendGroup = (typeof SPEND_GROUPS)[number];

/** One of a dashboard's charts: what it draws and, for spend, what splits it. */
export type Chart = { measure: ChartMeasure; group?: SpendGroup; tag?: string };

/** The cost dashboard the page opens on where none is named. */
export const COST_DASHBOARD: readonly Chart[] = [
  { measure: ChartMeasure.Spend },
  { measure: ChartMeasure.Tokens },
  { measure: ChartMeasure.Runs },
  { measure: ChartMeasure.Latency },
  { measure: ChartMeasure.FirstToken },
];

/** A chart as the page's URL carries it: `spend`, `spend:user` or
 *  `spend:tag:<key>`, the key whole after its second colon. */
export function chartWord(chart: Chart): string {
  if (!chart.group) return chart.measure;
  return chart.group === MetricsGroup.Tag
    ? `${chart.measure}:tag:${chart.tag}`
    : `${chart.measure}:${chart.group}`;
}

/** `word` as a chart, or `null` where the page draws none such. */
export function chartOf(word: string): Chart | null {
  const [measure, group, ...key] = word.split(":");
  if (group === undefined) {
    return (Object.values(ChartMeasure) as string[]).includes(measure ?? "")
      ? { measure: measure as ChartMeasure }
      : null;
  }
  if (measure !== ChartMeasure.Spend || !(SPEND_GROUPS as readonly string[]).includes(group)) return null;
  if (group !== MetricsGroup.Tag) return key.length === 0 ? { measure, group: group as SpendGroup } : null;
  // A tag key as a sort by it takes one, which is the server's rule for a key.
  const tag = key.join(":").trim();
  return readTagSort(`tag:${tag}`) ? { measure, group, tag } : null;
}

/** The charts the URL's `chart` values name, each once and in their order,
 *  at most [`DASHBOARD_CHARTS_MAX`]; `null` where it names none. */
export function chartsOf(given: string | string[] | undefined): Chart[] | null {
  const words = (Array.isArray(given) ? given : given === undefined ? [] : [given]).slice(0, 64);
  const charts: Chart[] = [];
  const seen = new Set<string>();
  for (const word of words) {
    const chart = chartOf(word);
    if (!chart || seen.has(chartWord(chart))) continue;
    seen.add(chartWord(chart));
    charts.push(chart);
    if (charts.length === DASHBOARD_CHARTS_MAX) break;
  }
  return charts.length > 0 ? charts : null;
}

/** The dashboards page over `days`, drawing `charts`, opened from the saved
 *  dashboard `view` where one is named. */
export function dashboardsHref(
  organization: string,
  slug: string,
  days: number,
  charts: readonly Chart[],
  view?: string,
): string {
  return listHref(organization, slug, "/dashboards", { days, view, chart: charts.map(chartWord) });
}

/** Whether two dashboards draw the same charts, in the same order. */
export function sameCharts(a: readonly Chart[], b: readonly Chart[]): boolean {
  return a.map(chartWord).join("\n") === b.map(chartWord).join("\n");
}

/** One bar of the charts. */
export type Bar = {
  at: string;
  spend: number;
  input: number;
  output: number;
  ok: number;
  failed: number;
  p50: number | null;
  p95: number | null;
  p99: number | null;
};

/** Each hour's or day's totals as a bar, oldest first. */
export function barsOf(points: readonly RollupPoint[]): Bar[] {
  return points.map((point) => ({
    at: point.at,
    spend: point.spend,
    input: point.input_tokens,
    output: point.output_tokens,
    ok: point.runs - point.failed_runs,
    failed: point.failed_runs,
    p50: point.run_duration_ms.p50,
    p95: point.run_duration_ms.p95,
    p99: point.run_duration_ms.p99,
  }));
}

/** What a point of a split names: a value, none, or the values summed past
 *  the server's [`ROLLUP_GROUP_TOP`]. */
export type GroupValue = { kind: "value"; value: string } | { kind: "none" } | { kind: "others" };

function valueOf(point: RollupPoint): GroupValue {
  if (point.others) return { kind: "others" };
  return point.value ? { kind: "value", value: point.value } : { kind: "none" };
}

function keyOf(value: GroupValue): string {
  return value.kind === "value" ? `=${value.value}` : value.kind;
}

/** One group's totals over the window. */
export type GroupTotal = {
  value: GroupValue;
  spend: number;
  tokens: number;
  runs: number;
  failedRuns: number;
};

/** Each group's totals over the window, the costliest first, the most
 *  tokens breaking a tie; spans carrying no value of it, and the values
 *  folded past the top, each a row of its own, after the rest. */
export function groupTotals(points: readonly RollupPoint[]): GroupTotal[] {
  const totals = new Map<string, GroupTotal>();
  for (const point of points) {
    const value = valueOf(point);
    const key = keyOf(value);
    const held = totals.get(key) ?? { value, spend: 0, tokens: 0, runs: 0, failedRuns: 0 };
    held.spend += point.spend;
    held.tokens += point.input_tokens + point.output_tokens;
    held.runs += point.runs;
    held.failedRuns += point.failed_runs;
    totals.set(key, held);
  }
  const named = (total: GroupTotal) => (total.value.kind === "value" ? 0 : 1);
  return [...totals.values()].sort(
    (a, b) => named(a) - named(b) || b.spend - a.spend || b.tokens - a.tokens,
  );
}

/** The most values a split chart colours: the palette's seven that stay apart
 *  side by side; the rest are drawn as one grey Other. */
export const SERIES_MAX = 7;

/** One of a split chart's series: its key in each bar, its words and colour. */
export type Series = { key: string; label: string; color: string };

/** A split's spend over time: a bar an hour or a day, a key a series. The
 *  values that spent most over the window each a colour of their own, in
 *  that order; the rest, with spans carrying none and the values the server
 *  folded, summed as Other. */
export function spendSeries(
  points: readonly RollupPoint[],
  label: (value: GroupValue) => string,
): { series: Series[]; bars: Record<string, string | number>[] } {
  const top = groupTotals(points)
    .filter((total) => total.value.kind === "value")
    .slice(0, SERIES_MAX);
  const series: Series[] = top.map((total, i) => ({
    key: `s${i + 1}`,
    label: label(total.value),
    color: `var(--series-${i + 1})`,
  }));
  const index = new Map(top.map((total, i) => [keyOf(total.value), `s${i + 1}`]));
  const bars = new Map<string, Record<string, string | number>>();
  let other = false;
  for (const point of points) {
    const key = index.get(keyOf(valueOf(point))) ?? "other";
    other ||= key === "other";
    const bar = bars.get(point.at) ?? { at: point.at };
    bar[key] = Number(bar[key] ?? 0) + point.spend;
    bars.set(point.at, bar);
  }
  if (other) series.push({ key: "other", label: "Other", color: "var(--mark-faint)" });
  return { series, bars: [...bars.values()] };
}
