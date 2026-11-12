"use client";

import { useState, type ComponentProps, type MouseEvent, type ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Cell, Line, LineChart, ReferenceLine, XAxis, YAxis } from "recharts";

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
import { openPeek } from "@/lib/peek";
import type { Trace, TraceHistogram } from "@/lib/server/entities/traces";
import { formatScoreValue } from "@/lib/scores";
import {
  FAULT_LABEL,
  STATUS_LABEL,
  formatCacheShare,
  formatCost,
  formatCount,
  formatDuration,
  formatOutputSpeed,
  shortId,
  tokenBreakdown,
  type ChangeMark,
  type TokenBar,
  type TokenCounts,
  type OutputSpeedBar,
} from "@/lib/traces";
import { SpanFault, type SpanStatus } from "@/lib/types/enums";
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

// Every chart at once, two by two, as an agent's page draws its traces: by how
// they ended, their tokens, how long they ran and how soon their first model
// call began to answer — the median and the slowest twentieth — and, where
// the read is narrowed to a score, that score's mean, each a bar of the same
// width.
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
        <ChartCard
          key={metric}
          label={METRIC_LABEL[metric]}
          title={metric === "score" ? `The mean ${histogram.score} score` : METRIC_LABEL[metric]}
          note={metric === "faults" ? "Traces containing each fault; categories can overlap." : undefined}
        >
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
        </ChartCard>
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

/** One trace of a session, as its charts read it. */
export type SessionTrace = Pick<
  Trace,
  | "trace_id"
  | "started_at"
  | "ended"
  | "status"
  | "duration_ms"
  | "spend"
  | "time_to_first_token_ms"
  | keyof TokenCounts
>;

/** How a trace stands on a session's duration chart: how it ended, or still
 *  running until its agent's own span arrives. */
type TraceState = SpanStatus | "running";
const TRACE_STATES: readonly TraceState[] = ["ok", "error", "cancelled", "running"];
const stateOf = (trace: Pick<Trace, "ended" | "status">): TraceState => (trace.ended ? trace.status : "running");
const stateLabel = (state: TraceState) => (state === "running" ? "Running" : STATUS_LABEL[state]);

/** A state's swatch, in the legend and the tooltip. A trace still running is
 *  hollow and dashed, as its bar is: a green of its own beside the failures'
 *  red would be one colour to a reader who tells neither apart. */
function StateSwatch({ state }: { state: TraceState }) {
  return state === "running" ? (
    <span className="size-2.5 shrink-0 rounded-xs border border-dashed border-muted-foreground" aria-hidden="true" />
  ) : (
    <span className="size-2.5 shrink-0 rounded-xs" style={{ background: TRACES[state]?.color }} aria-hidden="true" />
  );
}

const SESSION_DURATION: ChartConfig = { duration: { label: "Duration" } };
const SESSION_COST: ChartConfig = { spent: { label: "So far", color: "var(--foreground)" } };
const SESSION_FIRST_TOKEN: ChartConfig = {
  first_token: { label: "Time to first token", color: "var(--foreground)" },
};
const SESSION_CHART = "aspect-auto h-56 w-full";

// A session's own charts, a step a trace in the order they started, each as
// wide as the next: how long it ran and how it ended, its tokens as five, what
// the session had cost once it ended, and how soon its first model call began
// to answer. Steps rather than the clock's time, so the idle hours between
// two traces take no room; the axis names when each started. A click on a
// trace's step opens it in the page's peek, as its row below does.
export function SessionCharts({ traces }: { traces: readonly SessionTrace[] }) {
  const inBrowser = useInBrowser();
  // What the session had cost once each trace ended, in their order.
  const totals: number[] = [];
  for (const trace of traces) totals.push((totals.at(-1) ?? 0) + trace.spend);
  const steps = traces.map((trace, i) => ({
    id: trace.trace_id,
    at: trace.started_at,
    state: stateOf(trace),
    duration: trace.duration_ms,
    ...tokenBreakdown([trace]),
    cost: trace.spend,
    spent: totals[i] ?? 0,
    first_token: trace.time_to_first_token_ms,
  }));
  const byId = new Map(steps.map((step) => [step.id, step]));
  const [first, last] = [steps.at(0), steps.at(-1)];
  // A tick names its trace's hour while they all started on one day, else its day and hour.
  const oneDay = first && last && new Date(first.at).toDateString() === new Date(last.at).toDateString();
  // A click or a tap opens the trace whose band it landed in, read off the
  // plot's own box: the step recharts holds hovered is one tap behind on a
  // touch screen.
  const open = (event: MouseEvent<HTMLDivElement>) => {
    const plot = event.currentTarget.querySelector(".recharts-cartesian-grid")?.getBoundingClientRect();
    // Within the plot alone: its legend and its axes open nothing.
    if (!plot || plot.width <= 0 || event.clientY < plot.top || event.clientY > plot.bottom) return;
    const step = steps[Math.floor(((event.clientX - plot.left) / plot.width) * steps.length)];
    if (step) openPeek(step.id);
  };
  const frame = { data: steps, margin: { left: 0, right: 8, top: 4 } };
  const grid = <CartesianGrid vertical={false} />;
  // Bands on the line charts too, so a trace's dot stands over its bar in the
  // chart above, and the first and last are clear of the plot's edges.
  const axis = (
    <XAxis
      dataKey="id"
      scale="band"
      tickLine={false}
      axisLine={false}
      tickMargin={6}
      minTickGap={24}
      tickFormatter={(id: string) => {
        const step = byId.get(id);
        if (!step) return "";
        return oneDay ? tickLabel(step.at, 1) : barLabel(step.at, 0);
      }}
    />
  );
  // A step's tooltip is headed by when its trace started, and the trace's id.
  const tooltip = (formatter: NonNullable<ComponentProps<typeof ChartTooltipContent>["formatter"]>, bars = false) => (
    <ChartTooltip
      cursor={bars ? { fillOpacity: 0.5 } : true}
      content={
        <ChartTooltipContent
          labelFormatter={(_, payload) => {
            const step = byId.get(payload?.[0]?.payload?.id);
            return step ? `${barLabel(step.at, 0)} · ${shortId(step.id)}` : null;
          }}
          formatter={formatter}
        />
      }
    />
  );
  const row = (swatch: ReactNode, label: string, value: string) => (
    <div className="flex w-full items-center gap-2">
      {swatch}
      <span className="text-muted-foreground">{label}</span>
      <span className="ml-auto tabular-nums">{value}</span>
    </div>
  );
  const dot = <span className="size-2.5 shrink-0 rounded-xs bg-foreground" aria-hidden="true" />;
  const drawn = (chart: ReactNode) => (inBrowser ? chart : <div className={SESSION_CHART} aria-hidden="true" />);

  return (
    <div className="grid gap-page lg:grid-cols-2">
      <ChartCard title="Duration" note="How long each trace ran, and how it stands.">
        {/* As tall with its legend as the charts beside it, whose legends recharts draws inside them. */}
        <div className="flex h-56 flex-col gap-gutter">
          {inBrowser ? (
            <ChartContainer
              config={SESSION_DURATION}
              className="aspect-auto min-h-0 w-full flex-1 cursor-pointer"
              onClick={open}
            >
              <BarChart {...frame}>
                {grid}
                {axis}
                <YAxis width="auto" tickLine={false} axisLine={false} tickFormatter={formatDuration} />
                {tooltip(function Row(raw, _name, item) {
                  const step = byId.get(item.payload?.id);
                  if (!step || typeof raw !== "number") return null;
                  return row(<StateSwatch state={step.state} />, stateLabel(step.state), formatDuration(raw));
                }, true)}
                <Bar dataKey="duration" radius={2}>
                  {steps.map((step) =>
                    step.state === "running" ? (
                      <Cell key={step.id} fill="transparent" stroke="var(--muted-foreground)" strokeDasharray="3 2" />
                    ) : (
                      <Cell key={step.id} fill={TRACES[step.state]?.color} />
                    ),
                  )}
                </Bar>
              </BarChart>
            </ChartContainer>
          ) : (
            <div className="flex-1" aria-hidden="true" />
          )}
          <ul className="flex shrink-0 flex-wrap items-center justify-center gap-4 text-xs" aria-label="How its traces stand">
            {TRACE_STATES.filter((state) => steps.some((step) => step.state === state)).map((state) => (
              <li key={state} className="flex items-center gap-1.5">
                <StateSwatch state={state} />
                {stateLabel(state)}
              </li>
            ))}
          </ul>
        </div>
      </ChartCard>
      <ChartCard title="Tokens" note="Each trace's model calls' tokens.">
        {drawn(
          <ChartContainer config={TOKEN_SEGMENTS} className={cn(SESSION_CHART, "cursor-pointer")} onClick={open}>
            <BarChart {...frame}>
              {grid}
              {axis}
              <YAxis width="auto" tickLine={false} axisLine={false} allowDecimals={false} tickFormatter={formatCount} />
              {tooltip(tooltipRow(TOKEN_SEGMENTS, formatCount), true)}
              <ChartLegend content={<ChartLegendContent />} />
              {Object.keys(TOKEN_SEGMENTS).map((segment) => (
                <Bar key={segment} dataKey={segment} stackId="tokens" fill={`var(--color-${segment})`} />
              ))}
            </BarChart>
          </ChartContainer>,
        )}
      </ChartCard>
      <ChartCard title="Cost so far" note="What the session had cost as each trace ended.">
        {drawn(
          <ChartContainer config={SESSION_COST} className={cn(SESSION_CHART, "cursor-pointer")} onClick={open}>
            <LineChart {...frame}>
              {grid}
              {axis}
              <YAxis width="auto" tickLine={false} axisLine={false} tickFormatter={formatCost} />
              {tooltip(function Rows(raw, _name, item) {
                const step = byId.get(item.payload?.id);
                if (!step || typeof raw !== "number") return null;
                return (
                  <div className="grid w-full gap-1.5">
                    {row(dot, "This trace", formatCost(step.cost))}
                    {row(dot, "So far", formatCost(raw))}
                  </div>
                );
              })}
              <Line dataKey="spent" type="stepAfter" stroke="var(--color-spent)" strokeWidth={1.5} dot={{ r: 2 }} />
            </LineChart>
          </ChartContainer>,
        )}
      </ChartCard>
      <ChartCard
        title="Time to first token"
        note={
          steps.some((step) => step.first_token !== null)
            ? "How soon each trace's first model call began to answer."
            : "No trace of the session reported its time to first token."
        }
      >
        {drawn(
          <ChartContainer
            config={SESSION_FIRST_TOKEN}
            className={cn(SESSION_CHART, "cursor-pointer")}
            onClick={open}
          >
            <LineChart {...frame}>
              {grid}
              {axis}
              <YAxis width="auto" tickLine={false} axisLine={false} tickFormatter={formatDuration} />
              {tooltip(tooltipRow(SESSION_FIRST_TOKEN, formatDuration))}
              {/* Dots, so a trace between two that reported none is still drawn. */}
              <Line
                dataKey="first_token"
                type="linear"
                stroke="var(--color-first_token)"
                strokeWidth={1.5}
                dot={{ r: 2 }}
                connectNulls={false}
              />
            </LineChart>
          </ChartContainer>,
        )}
      </ChartCard>
    </div>
  );
}

/** One chart of a page's set, under its heading; named by `label` where its
 *  heading says more than its name. */
function ChartCard({
  title,
  label = title,
  note,
  children,
}: {
  title: string;
  label?: string;
  note?: string;
  children: ReactNode;
}) {
  return (
    <div role="region" className="grid gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border" aria-label={label}>
      <h2 className="text-base font-medium">{title}</h2>
      {note && <p className="text-muted-foreground">{note}</p>}
      {children}
    </div>
  );
}
