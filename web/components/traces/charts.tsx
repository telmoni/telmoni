"use client";

import { useState } from "react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, ReferenceLine, XAxis, YAxis } from "recharts";

import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { TraceHistogram } from "@/lib/server/entities/traces";
import { formatScoreValue } from "@/lib/scores";
import { formatCost, formatCount, formatDuration, type ChangeMark } from "@/lib/traces";
import { useInBrowser } from "@/lib/in-browser";
import { isOneOf } from "@/lib/one-of";
import { cn } from "@/lib/utils";

/** What a chart of the list draws. */
const METRICS = ["traces", "tokens", "cost", "duration", "first_token", "score"] as const;
type Metric = (typeof METRICS)[number];

/** The metrics `histogram` can draw: a score's mean only where the list is
 *  narrowed to one. */
function drawn(histogram: TraceHistogram): Metric[] {
  return METRICS.filter((metric) => metric !== "score" || histogram.score !== null);
}

const METRIC_LABEL: Record<Metric, string> = {
  traces: "Traces",
  tokens: "Tokens",
  cost: "Cost",
  duration: "Duration",
  first_token: "Time to first token",
  score: "Score",
};

// The theme's own colours, which follow it into the dark: a trace that ended
// well in the muted ink, a failure in the destructive red the status badge
// wears, a cancelled one fainter than either.
const TRACES: ChartConfig = {
  ok: { label: "OK", color: "var(--muted-foreground)" },
  error: { label: "Error", color: "var(--destructive)" },
  cancelled: { label: "Cancelled", color: "var(--ring)" },
};
const TOKENS: ChartConfig = {
  tokens: { label: "Tokens", color: "var(--muted-foreground)" },
};
const COST: ChartConfig = {
  cost: { label: "Cost", color: "var(--muted-foreground)" },
};
const SCORE: ChartConfig = {
  score: { label: "Mean", color: "var(--foreground)" },
};
const DURATION: ChartConfig = {
  p50: { label: "Median", color: "var(--muted-foreground)" },
  p95: { label: "95th percentile", color: "var(--foreground)" },
};

/** A bar's start as an axis names it: its hour on a day's chart, and its
 *  day on a longer one, whose hours would all read midnight where the axis
 *  has room for a label a day. */
function tickLabel(iso: string, days: number): string {
  const at = new Date(iso);
  return days <= 1
    ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString([], { month: "short", day: "numeric" });
}

/** A bar's start as its tooltip names it. */
function barLabel(iso: string, seconds: number): string {
  const at = new Date(iso);
  return seconds < 86_400
    ? at.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
}

/** The histogram's bars as the charts read them, the percentiles `metric`'s. */
function rows(histogram: TraceHistogram, metric: Metric) {
  return histogram.bars.map((bar) => ({
    at: bar.at,
    ok: bar.ok,
    error: bar.error,
    cancelled: bar.cancelled,
    tokens: bar.tokens,
    cost: bar.spend,
    p50: metric === "first_token" ? bar.time_to_first_token_p50_ms : bar.duration_p50_ms,
    p95: metric === "first_token" ? bar.time_to_first_token_p95_ms : bar.duration_p95_ms,
    score: bar.score_mean,
  }));
}

/** One metric over the window, in the histogram's bars. */
function MetricChart({
  histogram,
  marks,
  metric,
  className,
  legend = false,
}: {
  histogram: TraceHistogram;
  marks: readonly ChangeMark[];
  metric: Metric;
  className?: string;
  legend?: boolean;
}) {
  const inBrowser = useInBrowser();
  if (!inBrowser) return <div className={cn("w-full", className)} aria-hidden="true" />;

  const data = rows(histogram, metric);
  const seconds = histogram.bucket_seconds;
  const changed = new Map(marks.map((mark) => [mark.at, mark.changes]));
  const lines = metric === "duration" || metric === "first_token" || metric === "score";
  const config =
    metric === "traces"
      ? TRACES
      : metric === "tokens"
        ? TOKENS
        : metric === "cost"
          ? COST
          : metric === "score"
            ? SCORE
            : DURATION;
  const value = (v: number) =>
    metric === "score"
      ? formatScoreValue(v)
      : lines
        ? formatDuration(v)
        : metric === "cost"
          ? formatCost(v)
          : formatCount(v);
  // A longer window's bars narrower than a day are labelled where a day
  // starts alone, so its dates step along the axis rather than repeat.
  const dayStarts =
    histogram.days > 1 && seconds < 86_400
      ? data.filter((row) => new Date(row.at).getHours() === 0).map((row) => row.at)
      : undefined;
  const axis = (
    <XAxis
      dataKey="at"
      ticks={dayStarts}
      tickLine={false}
      axisLine={false}
      tickMargin={6}
      minTickGap={24}
      tickFormatter={(iso: string) => tickLabel(iso, histogram.days)}
    />
  );
  const tooltip = (
    <ChartTooltip
      cursor={lines ? true : { fillOpacity: 0.5 }}
      content={
        <ChartTooltipContent
          labelFormatter={(_, payload) => {
            const iso = payload?.[0]?.payload?.at;
            if (typeof iso !== "string") return null;
            const changes = changed.get(iso);
            if (!changes) return barLabel(iso, seconds);
            return (
              <div className="grid max-w-80 gap-1">
                {barLabel(iso, seconds)}
                {changes.map((line, i) => (
                  <div key={i} className="flex gap-2 font-normal">
                    <span className="shrink-0 border-l border-dashed border-foreground" aria-hidden="true" />
                    <span className="min-w-0 wrap-break-word">{line}</span>
                  </div>
                ))}
              </div>
            );
          }}
          formatter={(raw, name, item) => (
            <div className="flex w-full items-center gap-2">
              <span
                className="size-2.5 shrink-0 rounded-xs"
                style={{ background: item.color ?? item.payload?.fill }}
              />
              <span className="text-muted-foreground">{config[String(name)]?.label ?? name}</span>
              <span className="ml-auto tabular-nums">
                {typeof raw === "number" ? value(raw) : "—"}
              </span>
            </div>
          )}
        />
      }
    />
  );
  const grid = <CartesianGrid vertical={false} />;
  const yAxis = (
    <YAxis
      // Measured from the labels drawn, not guessed per metric: a fixed
      // forty pixels cut the first digit off a tokens axis past 9,999.
      width="auto"
      tickLine={false}
      axisLine={false}
      allowDecimals={metric === "cost" || metric === "score"}
      tickFormatter={(v: number) => value(v)}
    />
  );
  // Dashed apart from the solid series, and thin, so a bar it crosses stays whole.
  const changeLines = marks.map((mark) => (
    <ReferenceLine key={mark.at} x={mark.at} stroke="var(--foreground)" strokeDasharray="3 3" />
  ));

  return (
    <ChartContainer config={config} className={cn("aspect-auto w-full", className)}>
      {lines ? (
        <LineChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          {metric === "score" ? (
            // Dotted too: a bar between empty ones has no neighbour to draw a line to.
            <Line dataKey="score" type="monotone" stroke="var(--color-score)" strokeWidth={2} dot={{ r: 4 }} connectNulls={false} />
          ) : (
            <>
              <Line dataKey="p50" type="monotone" stroke="var(--color-p50)" strokeWidth={1.5} dot={false} connectNulls={false} />
              <Line dataKey="p95" type="monotone" stroke="var(--color-p95)" strokeWidth={1.5} dot={false} connectNulls={false} />
            </>
          )}
          {changeLines}
        </LineChart>
      ) : (
        <BarChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          {metric === "tokens" || metric === "cost" ? (
            <Bar dataKey={metric} fill={`var(--color-${metric})`} radius={2} />
          ) : (
            <>
              <Bar dataKey="ok" stackId="traces" fill="var(--color-ok)" />
              <Bar dataKey="cancelled" stackId="traces" fill="var(--color-cancelled)" />
              <Bar dataKey="error" stackId="traces" fill="var(--color-error)" radius={[2, 2, 0, 0]} />
            </>
          )}
          {changeLines}
        </BarChart>
      )}
    </ChartContainer>
  );
}

// The strip above the table:
// the window's traces by when they started, or their tokens or durations,
// picked beside it. A region rather than a `<section>`, which the console
// keeps for a page's own blocks.
export function TraceHistogramStrip({
  histogram,
  marks,
}: {
  histogram: TraceHistogram;
  marks: readonly ChangeMark[];
}) {
  const [picked, setMetric] = useState<Metric>("traces");
  // A score picked, then its filter cleared, falls back to the traces.
  const metric = drawn(histogram).includes(picked) ? picked : "traces";
  return (
    <div role="region" className="grid gap-1" aria-label="Over the window">
      <div className="flex items-center gap-gutter">
        <Select
          value={metric}
          onValueChange={(next) => {
            if (isOneOf(METRICS, next)) setMetric(next);
          }}
        >
          {/* The console's control height, so its row centers on the Filters heading beside it. */}
          <SelectTrigger className="border-none shadow-none" aria-label="What the chart shows">
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="start">
            {drawn(histogram).map((m) => (
              <SelectItem key={m} value={m}>
                {METRIC_LABEL[m]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <MetricChart histogram={histogram} marks={marks} metric={metric} className="h-28" />
    </div>
  );
}

// The charts in the table's place, two by two: the window's traces by how they
// ended, their tokens, how long they ran and how soon their first model call
// began to answer — the median and the slowest twentieth — and, where the
// list is narrowed to a score, that score's mean, each a bar of the same width.
export function TraceCharts({
  histogram,
  marks,
}: {
  histogram: TraceHistogram;
  marks: readonly ChangeMark[];
}) {
  return (
    <div className="grid gap-page lg:grid-cols-2">
      {drawn(histogram).map((metric) => (
        <div
          key={metric}
          role="region"
          className="grid gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border"
          aria-label={METRIC_LABEL[metric]}
        >
          <h2 className="text-base font-medium">
            {metric === "score" ? `The mean ${histogram.score} score` : METRIC_LABEL[metric]}
          </h2>
          <MetricChart histogram={histogram} marks={marks} metric={metric} className="h-56" legend={metric !== "tokens" && metric !== "cost"} />
        </div>
      ))}
    </div>
  );
}
