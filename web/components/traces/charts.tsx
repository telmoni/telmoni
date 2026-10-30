"use client";

import { useState } from "react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

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
import { formatCount, formatDuration } from "@/lib/traces";
import { useInBrowser } from "@/lib/in-browser";
import { isOneOf } from "@/lib/one-of";
import { cn } from "@/lib/utils";

/** What a chart of the list draws. */
const METRICS = ["runs", "tokens", "duration", "first_token"] as const;
type Metric = (typeof METRICS)[number];

const METRIC_LABEL: Record<Metric, string> = {
  runs: "Runs",
  tokens: "Tokens",
  duration: "Duration",
  first_token: "Time to first token",
};

// The theme's own colours, which follow it into the dark: a run that ended
// well in the muted ink, a failure in the destructive red the status badge
// wears, a cancelled one fainter than either.
const RUNS: ChartConfig = {
  ok: { label: "OK", color: "var(--muted-foreground)" },
  error: { label: "Error", color: "var(--destructive)" },
  cancelled: { label: "Cancelled", color: "var(--ring)" },
};
const TOKENS: ChartConfig = {
  tokens: { label: "Tokens", color: "var(--muted-foreground)" },
};
const DURATION: ChartConfig = {
  p50: { label: "Median", color: "var(--muted-foreground)" },
  p95: { label: "95th percentile", color: "var(--foreground)" },
};


/** A bar's width in words: "an hour a bar", "3 hours a bar", "a day a bar". */
function perBar(seconds: number): string {
  if (seconds === 604_800) return "a week a bar";
  if (seconds === 86_400) return "a day a bar";
  const hours = seconds / 3_600;
  return hours === 1 ? "an hour a bar" : `${hours} hours a bar`;
}

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
    p50: metric === "first_token" ? bar.time_to_first_token_p50_ms : bar.duration_p50_ms,
    p95: metric === "first_token" ? bar.time_to_first_token_p95_ms : bar.duration_p95_ms,
  }));
}

/** One metric over the window, in the histogram's bars. */
function MetricChart({
  histogram,
  metric,
  className,
  legend = false,
}: {
  histogram: TraceHistogram;
  metric: Metric;
  className?: string;
  legend?: boolean;
}) {
  const inBrowser = useInBrowser();
  if (!inBrowser) return <div className={cn("w-full", className)} aria-hidden="true" />;

  const data = rows(histogram, metric);
  const seconds = histogram.bucket_seconds;
  const lines = metric === "duration" || metric === "first_token";
  const config = metric === "runs" ? RUNS : metric === "tokens" ? TOKENS : DURATION;
  const value = (v: number) => (lines ? formatDuration(v) : formatCount(v));
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
            return typeof iso === "string" ? barLabel(iso, seconds) : null;
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
      allowDecimals={false}
      tickFormatter={(v: number) => value(v)}
    />
  );

  return (
    <ChartContainer config={config} className={cn("aspect-auto w-full", className)}>
      {lines ? (
        <LineChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          <Line dataKey="p50" type="monotone" stroke="var(--color-p50)" strokeWidth={1.5} dot={false} connectNulls={false} />
          <Line dataKey="p95" type="monotone" stroke="var(--color-p95)" strokeWidth={1.5} dot={false} connectNulls={false} />
        </LineChart>
      ) : (
        <BarChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          {metric === "tokens" ? (
            <Bar dataKey="tokens" fill="var(--color-tokens)" radius={2} />
          ) : (
            <>
              <Bar dataKey="ok" stackId="runs" fill="var(--color-ok)" />
              <Bar dataKey="cancelled" stackId="runs" fill="var(--color-cancelled)" />
              <Bar dataKey="error" stackId="runs" fill="var(--color-error)" radius={[2, 2, 0, 0]} />
            </>
          )}
        </BarChart>
      )}
    </ChartContainer>
  );
}

// The strip above the table:
// the window's runs by when they started, or their tokens or durations,
// picked beside it. A region rather than a `<section>`, which the console
// keeps for a page's own blocks.
export function TraceHistogramStrip({ histogram }: { histogram: TraceHistogram }) {
  const [metric, setMetric] = useState<Metric>("runs");
  return (
    <div role="region" className="grid gap-1" aria-label="Over the window">
      <div className="flex items-center gap-2">
        <Select
          value={metric}
          onValueChange={(next) => {
            if (isOneOf(METRICS, next)) setMetric(next);
          }}
        >
          {/* The console's control height, so its row centers on the Filters heading beside it. */}
          <SelectTrigger size="sm" className="border-none px-2 shadow-none" aria-label="What the chart shows">
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="start">
            {METRICS.map((m) => (
              <SelectItem key={m} value={m}>
                {METRIC_LABEL[m]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="text-xs text-muted-foreground">
          {perBar(histogram.bucket_seconds)}
        </span>
      </div>
      <MetricChart histogram={histogram} metric={metric} className="h-28" />
    </div>
  );
}

// The charts in the table's place, two by two: the window's runs by how they
// ended, their tokens, how long they ran and how soon their first model call
// began to answer — the median and the slowest twentieth — each a bar of the
// same width.
export function TraceCharts({ histogram }: { histogram: TraceHistogram }) {
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      {METRICS.map((metric) => (
        <div
          key={metric}
          role="region"
          className="grid gap-2 rounded-xl border border-border p-4"
          aria-label={METRIC_LABEL[metric]}
        >
          <h2 className="text-sm font-medium">
            {METRIC_LABEL[metric]}
            <span className="ml-2 font-normal text-muted-foreground">
              {perBar(histogram.bucket_seconds)}
            </span>
          </h2>
          <MetricChart histogram={histogram} metric={metric} className="h-56" legend={metric !== "tokens"} />
        </div>
      ))}
    </div>
  );
}
