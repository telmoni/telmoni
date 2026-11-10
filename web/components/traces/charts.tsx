"use client";

import { useState, type ComponentProps } from "react";
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
import {
  FAULT_LABEL,
  formatCacheShare,
  formatCost,
  formatCount,
  formatDuration,
  formatOutputSpeed,
  tokenBreakdown,
  type ChangeMark,
  type TokenBar,
  type OutputSpeedBar,
} from "@/lib/traces";
import { SpanFault } from "@/lib/types/enums";
import { useInBrowser } from "@/lib/in-browser";
import { isOneOf } from "@/lib/one-of";
import { cn } from "@/lib/utils";

/** What a chart of the list draws. */
const METRICS = ["traces", "faults", "tokens", "cost", "duration", "first_token", "output_speed", "score"] as const;
type Metric = (typeof METRICS)[number];

/** The metrics `histogram` can draw: a score's mean only where the list is
 *  narrowed to one. */
function drawn(histogram: TraceHistogram): Metric[] {
  return METRICS.filter((metric) => metric !== "score" || histogram.score !== null);
}

const METRIC_LABEL: Record<Metric, string> = {
  traces: "Traces",
  faults: "Traces with faults",
  tokens: "Tokens",
  cost: "Cost",
  duration: "Duration",
  first_token: "Time to first token",
  output_speed: "Output speed",
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
/** A class's dashes, its own on every chart, so a line reads the same whichever others are drawn. */
function faultDash(fault: SpanFault): string {
  const place = Object.values(SpanFault).indexOf(fault) + 1;
  return `${place} ${place}`;
}
const FAULTS: ChartConfig = Object.fromEntries(
  Object.values(SpanFault).map((fault) => [
    fault,
    {
      label: FAULT_LABEL[fault],
      color: "var(--foreground)",
      icon: () => (
        <svg viewBox="0 0 32 16" className="h-4 w-8" aria-hidden="true">
          <path d="M0 8H32" fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray={faultDash(fault)} />
        </svg>
      ),
    },
  ]),
);
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
const OUTPUT_SPEED: ChartConfig = {
  p50: { label: "Median", color: "var(--muted-foreground)" },
  p05: { label: "5th percentile", color: "var(--foreground)" },
};
// Five steps of the ink, darkest first, so the stack reads in either theme.
const ink = (share: number) => `color-mix(in oklab, var(--foreground) ${share}%, var(--background))`;
const TOKEN_SEGMENTS: ChartConfig = {
  input: { label: "Input", color: ink(100) },
  cacheRead: { label: "Cache read", color: ink(80) },
  cacheWrite: { label: "Cache write", color: ink(60) },
  output: { label: "Output", color: ink(40) },
  reasoning: { label: "Reasoning", color: ink(20) },
};

/** A bar's start as an axis names it: its hour on a day's chart, and its
 *  day on a longer one, whose hours would all read midnight where the axis
 *  has room for a label a day. */
function tickLabel(iso: string, days: number, timeZone?: "UTC"): string {
  const at = new Date(iso);
  return days <= 1
    ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", timeZone })
    : at.toLocaleDateString([], { month: "short", day: "numeric", timeZone });
}

/** A bar's start as its tooltip names it. */
function barLabel(iso: string, seconds: number, timeZone?: "UTC"): string {
  const at = new Date(iso);
  return seconds < 86_400
    ? at.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", timeZone })
    : at.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric", timeZone });
}

/** A bar's tooltip label: its start, and under it each change that began in it. */
function markedLabel(iso: string, seconds: number, marks: readonly ChangeMark[], timeZone?: "UTC") {
  const changes = marks.find((mark) => mark.at === iso)?.changes;
  if (!changes) return barLabel(iso, seconds, timeZone);
  return (
    <div className="grid max-w-80 gap-1">
      {barLabel(iso, seconds, timeZone)}
      {changes.map((line, i) => (
        <div key={i} className="flex gap-2 font-normal">
          <span className="shrink-0 border-l border-dashed border-foreground" aria-hidden="true" />
          <span className="min-w-0 wrap-break-word">{line}</span>
        </div>
      ))}
    </div>
  );
}

/** A tooltip's row: the series' colour and label, and its value as the chart's axis writes it. */
function tooltipRow(
  config: ChartConfig,
  format: (value: number) => string,
): NonNullable<ComponentProps<typeof ChartTooltipContent>["formatter"]> {
  return function Row(raw, name, item) {
    return (
      <div className="flex w-full items-center gap-2">
        <span className="size-2.5 shrink-0 rounded-xs" style={{ background: item.color ?? item.payload?.fill }} />
        <span className="text-muted-foreground">{config[String(name)]?.label ?? name}</span>
        <span className="ml-auto tabular-nums">{typeof raw === "number" ? format(raw) : "—"}</span>
      </div>
    );
  };
}

// Dashed apart from the solid series, and thin, so a bar it crosses stays whole.
function changeLines(marks: readonly ChangeMark[]) {
  return marks.map((mark) => (
    <ReferenceLine key={mark.at} x={mark.at} stroke="var(--foreground)" strokeDasharray="3 3" />
  ));
}

/** The histogram's bars as the charts read them, the percentiles `metric`'s. */
function rows(histogram: TraceHistogram, metric: Exclude<Metric, "tokens" | "output_speed">) {
  return histogram.bars.map((bar) => ({
    at: bar.at,
    ok: bar.ok,
    error: bar.error,
    cancelled: bar.cancelled,
    ...Object.fromEntries(Object.values(SpanFault).map((fault) => [fault, bar.faults[fault] ?? 0])),
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
  metric: Exclude<Metric, "tokens" | "output_speed">;
  className?: string;
  legend?: boolean;
}) {
  const inBrowser = useInBrowser();
  if (!inBrowser) return <div className={cn("w-full", className)} aria-hidden="true" />;

  const data = rows(histogram, metric);
  const seconds = histogram.bucket_seconds;
  const lines =
    metric === "duration" || metric === "first_token" || metric === "score" || metric === "faults";
  // Only the classes the window holds: a class nobody met is no line and no legend entry.
  const faults = Object.values(SpanFault).filter((fault) =>
    histogram.bars.some((bar) => (bar.faults[fault] ?? 0) > 0),
  );
  const config =
    metric === "traces"
      ? TRACES
      : metric === "faults"
        ? Object.fromEntries(faults.map((fault) => [fault, FAULTS[fault]!]))
        : metric === "cost"
          ? COST
          : metric === "score"
            ? SCORE
            : DURATION;
  const value = (v: number) =>
    metric === "score"
      ? formatScoreValue(v)
      : metric === "faults" ? formatCount(v)
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
            return markedLabel(iso, seconds, marks);
          }}
          formatter={tooltipRow(config, value)}
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
  return (
    <ChartContainer config={config} className={cn("aspect-auto w-full", className)}>
      {lines ? (
        <LineChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          {metric === "faults" ? (
            faults.map((fault) => (
              <Line
                key={fault}
                dataKey={fault}
                type="monotone"
                stroke="var(--foreground)"
                strokeDasharray={faultDash(fault)}
                strokeWidth={1.5}
                dot={false}
                connectNulls={false}
              />
            ))
          ) : metric === "score" ? (
            // Dotted too: a bar between empty ones has no neighbour to draw a line to.
            <Line dataKey="score" type="monotone" stroke="var(--color-score)" strokeWidth={2} dot={{ r: 4 }} connectNulls={false} />
          ) : (
            <>
              <Line dataKey="p50" type="monotone" stroke="var(--color-p50)" strokeWidth={1.5} dot={false} connectNulls={false} />
              <Line dataKey="p95" type="monotone" stroke="var(--color-p95)" strokeWidth={1.5} dot={false} connectNulls={false} />
            </>
          )}
          {changeLines(marks)}
        </LineChart>
      ) : (
        <BarChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
          {grid}
          {axis}
          {yAxis}
          {tooltip}
          {legend && <ChartLegend content={<ChartLegendContent />} />}
          {metric === "cost" ? (
            <Bar dataKey="cost" fill="var(--color-cost)" radius={2} />
          ) : (
            <>
              <Bar dataKey="ok" stackId="traces" fill="var(--color-ok)" />
              <Bar dataKey="cancelled" stackId="traces" fill="var(--color-cancelled)" />
              <Bar dataKey="error" stackId="traces" fill="var(--color-error)" radius={[2, 2, 0, 0]} />
            </>
          )}
          {changeLines(marks)}
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
  models,
}: {
  histogram: TraceHistogram;
  marks: readonly ChangeMark[];
  models?: readonly string[];
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
      {metric === "faults" && <p className="text-muted-foreground">Traces containing each fault; one trace may appear in several categories.</p>}
      {metric === "tokens" ? (
        <TokenChart
          bars={histogram.bars}
          models={models}
          days={histogram.days}
          bucketSeconds={histogram.bucket_seconds}
          marks={marks}
          className="h-28"
        />
      ) : metric === "output_speed" ? (
        <OutputSpeedChart
          bars={histogram.bars}
          days={histogram.days}
          bucketSeconds={histogram.bucket_seconds}
          marks={marks}
          className="h-28"
        />
      ) : (
        <MetricChart histogram={histogram} marks={marks} metric={metric} className="h-28" legend={metric === "faults"} />
      )}
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
  models,
}: {
  histogram: TraceHistogram;
  marks: readonly ChangeMark[];
  models?: readonly string[];
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
          {metric === "faults" && <p className="text-muted-foreground">Traces containing each fault; categories can overlap.</p>}
          {metric === "tokens" ? (
            <TokenChart
              bars={histogram.bars}
              models={models}
              days={histogram.days}
              bucketSeconds={histogram.bucket_seconds}
              marks={marks}
              className="h-56"
            />
          ) : metric === "output_speed" ? (
            <OutputSpeedChart
              bars={histogram.bars}
              days={histogram.days}
              bucketSeconds={histogram.bucket_seconds}
              marks={marks}
              className="h-56"
            />
          ) : (
            <MetricChart
              histogram={histogram}
              marks={marks}
              metric={metric}
              className="h-56"
              legend={metric !== "cost"}
            />
          )}
        </div>
      ))}
    </div>
  );
}

const ALL_MODELS = "all";
// Prefixed, so a model named as the whole is still its own choice.
const modelChoice = (model: string) => `model:${model}`;

/** The models a chart offers; one model, or none named, leaves nothing to pick between. */
function ModelSelect({
  models,
  value,
  onValueChange,
  label,
  all = false,
}: {
  models: readonly string[];
  value: string;
  onValueChange: (value: string) => void;
  label: string;
  /** Whether every model together is a choice. */
  all?: boolean;
}) {
  if (models.length <= 1) return null;
  return (
    <Select value={value} onValueChange={onValueChange}>
      <SelectTrigger aria-label={label}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent position="popper" align="start">
        {all && <SelectItem value={ALL_MODELS}>All models</SelectItem>}
        {models.map((model) => (
          <SelectItem key={model} value={modelChoice(model)}>
            {model || "No model"}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

// One model's calls at a time, since percentiles of two models cannot be
// joined; a bar with no measured call is a gap, never a zero.
export function OutputSpeedChart({
  bars,
  days,
  bucketSeconds,
  marks = [],
  timeZone,
  className,
}: {
  bars: readonly OutputSpeedBar[];
  days: number;
  bucketSeconds: number;
  marks?: readonly ChangeMark[];
  /** Set where a bar starts on the UTC clock, as Spend's do. */
  timeZone?: "UTC";
  className?: string;
}) {
  const inBrowser = useInBrowser();
  const [picked, setPicked] = useState<string | null>(null);
  // `Object.hasOwn`, so a model named as a property of every object is still data.
  const speedOf = (bar: OutputSpeedBar, model: string) =>
    Object.hasOwn(bar.output_speed_by_model, model) ? bar.output_speed_by_model[model] : undefined;
  const models = [...new Set(bars.flatMap((bar) => Object.keys(bar.output_speed_by_model)))].sort();
  const measured = models.find((model) => bars.some((bar) => speedOf(bar, model)?.p50 != null));
  // The reader's pick while it is in the window, else the first model with a speed.
  const selected = models.find((model) => modelChoice(model) === picked) ?? measured ?? models[0] ?? "";
  const data = bars.map((bar) => {
    const speed = speedOf(bar, selected);
    return { at: bar.at, p50: speed?.p50 ?? null, p05: speed?.p05 ?? null };
  });
  const latest = data.findLast((bar) => bar.p50 !== null);
  const speedOrNone = (speed: number | null | undefined) => (speed == null ? "none" : formatOutputSpeed(speed));

  return (
    <div className="grid min-w-0 gap-gutter">
      <ModelSelect
        models={models}
        value={modelChoice(selected)}
        onValueChange={setPicked}
        label="Output speed model"
      />
      {latest ? (
        <p className="sr-only">
          {selected || "No model"}: latest median {speedOrNone(latest.p50)}, 5th percentile{" "}
          {speedOrNone(latest.p05)}.
        </p>
      ) : (
        <p className="text-muted-foreground">No model call in the window reported its time to first token.</p>
      )}
      {inBrowser ? (
        <ChartContainer config={OUTPUT_SPEED} className={cn("aspect-auto w-full", className)}>
          <LineChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="at"
              tickLine={false}
              axisLine={false}
              tickMargin={6}
              minTickGap={24}
              tickFormatter={(iso: string) => tickLabel(iso, days, timeZone)}
            />
            <YAxis width="auto" tickLine={false} axisLine={false} tickFormatter={formatOutputSpeed} />
            <ChartTooltip
              content={
                <ChartTooltipContent
                  labelFormatter={(_, payload) => {
                    const iso = payload?.[0]?.payload?.at;
                    return typeof iso === "string" ? markedLabel(iso, bucketSeconds, marks, timeZone) : null;
                  }}
                  formatter={tooltipRow(OUTPUT_SPEED, formatOutputSpeed)}
                />
              }
            />
            <ChartLegend content={<ChartLegendContent />} />
            {/* Dots, so a lone bar between two gaps is still drawn. */}
            {Object.keys(OUTPUT_SPEED).map((key) => (
              <Line
                key={key}
                dataKey={key}
                type="monotone"
                stroke={`var(--color-${key})`}
                strokeWidth={1.5}
                dot={{ r: 2 }}
                connectNulls={false}
              />
            ))}
            {changeLines(marks)}
          </LineChart>
        </ChartContainer>
      ) : (
        <div className={className} aria-hidden="true" />
      )}
    </div>
  );
}

// The tokens as five, stacked: input no cache served, the cache's reads and
// writes, output and reasoning, of every model or the one picked, with the
// share of input the cache answered beside them. One model at a time, since
// five segments a model would be read by nobody.
export function TokenChart({
  bars,
  days,
  bucketSeconds = days <= 1 ? 3_600 : 86_400,
  marks = [],
  models: listed = [],
  timeZone,
  className,
}: {
  bars: readonly TokenBar[];
  days: number;
  bucketSeconds?: number;
  marks?: readonly ChangeMark[];
  /** Models to offer beside those the bars name. */
  models?: readonly string[];
  /** Set where a day's bar starts on the UTC day, as the hourly totals' do. */
  timeZone?: "UTC";
  className?: string;
}) {
  const inBrowser = useInBrowser();
  const [picked, setPicked] = useState(ALL_MODELS);
  const models = [...new Set([...listed, ...bars.flatMap((bar) => Object.keys(bar.tokens_by_model))])].sort();
  // A model that left the window falls back to the whole.
  const selected = models.some((model) => modelChoice(model) === picked) ? picked : ALL_MODELS;
  const countsOf = (bar: TokenBar) =>
    Object.entries(bar.tokens_by_model)
      .filter(([model]) => selected === ALL_MODELS || modelChoice(model) === selected)
      .map(([, counts]) => counts);
  const data = bars.map((bar) => ({ at: bar.at, ...tokenBreakdown(countsOf(bar)) }));
  const totals = tokenBreakdown(bars.flatMap(countsOf));
  const share = formatCacheShare(totals.cacheShare);

  return (
    <div className="grid min-w-0 gap-gutter">
      <div className="flex flex-wrap items-center gap-gutter">
        <ModelSelect models={models} value={selected} onValueChange={setPicked} label="Token model" all />
        <span className="text-muted-foreground tabular-nums">Cache share {share}</span>
      </div>
      <p className="sr-only">
        {formatCount(totals.input)} input, {formatCount(totals.cacheRead)} cache read,{" "}
        {formatCount(totals.cacheWrite)} cache write, {formatCount(totals.output)} output and{" "}
        {formatCount(totals.reasoning)} reasoning tokens. Cache share {share}.
      </p>
      {inBrowser ? (
        <ChartContainer config={TOKEN_SEGMENTS} className={cn("aspect-auto w-full", className)}>
          <BarChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
            <CartesianGrid vertical={false} />
            <XAxis
              dataKey="at"
              tickLine={false}
              axisLine={false}
              tickMargin={6}
              minTickGap={24}
              tickFormatter={(iso: string) => tickLabel(iso, days, timeZone)}
            />
            <YAxis width="auto" tickLine={false} axisLine={false} allowDecimals={false} tickFormatter={formatCount} />
            <ChartTooltip
              cursor={{ fillOpacity: 0.5 }}
              content={
                <ChartTooltipContent
                  labelFormatter={(_, payload) => {
                    const iso = payload?.[0]?.payload?.at;
                    return typeof iso === "string" ? markedLabel(iso, bucketSeconds, marks, timeZone) : null;
                  }}
                  formatter={tooltipRow(TOKEN_SEGMENTS, formatCount)}
                />
              }
            />
            <ChartLegend content={<ChartLegendContent />} />
            {Object.keys(TOKEN_SEGMENTS).map((segment) => (
              <Bar key={segment} dataKey={segment} stackId="tokens" fill={`var(--color-${segment})`} />
            ))}
            {changeLines(marks)}
          </BarChart>
        </ChartContainer>
      ) : (
        <div className={className} aria-hidden="true" />
      )}
    </div>
  );
}
