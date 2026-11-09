"use client";

import Link from "next/link";
import { useId, type ReactNode } from "react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

import { Row, Rows } from "@/components/rows";

import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import {
  chartWord,
  groupTotals,
  spendSeries,
  type Chart as DashboardChart,
  type Bar as DashboardBar,
  type GroupValue,
} from "@/lib/dashboards";
import { useInBrowser } from "@/lib/in-browser";
import type { RollupPoint } from "@/lib/server/entities/dashboards";
import { formatCost, formatCount, formatDuration, sessionRunsHref, tracesHref } from "@/lib/traces";
import { ChartMeasure, MetricsGroup } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

import { chartTitle } from "./_labels";

/** One bar of time to first token, which only the spans keep. */
export type FirstTokenBar = {
  at: string;
  firstTokenP50: number | null;
  firstTokenP95: number | null;
};

/** A split chart's read: its points and how far back it read, or none. */
export type Split = { kind: "ok"; points: RollupPoint[]; days: number } | { kind: "unread" };

// The theme's own colours, as the trace list's charts take them.
const SPEND: ChartConfig = { spend: { label: "Spend", color: "var(--muted-foreground)" } };
const TOKENS: ChartConfig = {
  input: { label: "Input", color: "var(--muted-foreground)" },
  output: { label: "Output", color: "var(--foreground)" },
};
const RUNS: ChartConfig = {
  ok: { label: "OK", color: "var(--muted-foreground)" },
  failed: { label: "Failed", color: "var(--destructive)" },
};
const LATENCY: ChartConfig = {
  p50: { label: "Median", color: "var(--muted-foreground)" },
  p95: { label: "95th percentile", color: "var(--foreground)" },
  p99: { label: "99th percentile", color: "var(--destructive)" },
};
const FIRST_TOKEN: ChartConfig = {
  firstTokenP50: { label: "Median", color: "var(--muted-foreground)" },
  firstTokenP95: { label: "95th percentile", color: "var(--foreground)" },
};

/** An axis tick: the hour for a day's window, else the UTC day its bar starts. */
function tick(iso: string, hourly: boolean): string {
  const at = new Date(iso);
  return hourly
    ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString([], { month: "short", day: "numeric", timeZone: "UTC" });
}

function Chart({
  title,
  summary,
  config,
  bars,
  hourly,
  format,
  kind,
  wide,
  children,
}: {
  title: string;
  /** What the chart shows, in words, for a reader who cannot see it. */
  summary: string;
  config: ChartConfig;
  bars: readonly object[];
  hourly: boolean;
  format: (value: number) => string;
  kind: "bars" | "stacked" | "lines";
  /** Across both columns, as a split with its table is. */
  wide?: boolean;
  /** What follows the chart: a split's totals. */
  children?: ReactNode;
}) {
  const inBrowser = useInBrowser();
  const keys = Object.keys(config);
  const body = (
    <>
      <CartesianGrid vertical={false} />
      <XAxis
        dataKey="at"
        tickLine={false}
        axisLine={false}
        tickMargin={6}
        minTickGap={24}
        tickFormatter={(iso: string) => tick(iso, hourly)}
      />
      <YAxis width="auto" tickLine={false} axisLine={false} tickFormatter={(v: number) => format(v)} />
      <ChartTooltip content={<ChartTooltipContent />} />
      {keys.length > 1 && <ChartLegend content={<ChartLegendContent />} />}
    </>
  );
  const heading = useId();
  return (
    <section
      aria-labelledby={heading}
      className={cn("grid gap-gutter rounded-md border border-border p-gutter", wide && "lg:col-span-2")}
    >
      <h2 id={heading} className="text-base font-medium">
        {title}
      </h2>
      <p className="sr-only">{summary}</p>
      {inBrowser ? (
        <ChartContainer config={config} className="aspect-auto h-48 w-full">
          {kind === "lines" ? (
            <LineChart data={bars} margin={{ left: 0, right: 8, top: 4 }}>
              {body}
              {keys.map((key) => (
                <Line
                  key={key}
                  dataKey={key}
                  type="monotone"
                  stroke={`var(--color-${key})`}
                  strokeWidth={1.5}
                  dot={false}
                  connectNulls={false}
                />
              ))}
            </LineChart>
          ) : (
            <BarChart data={bars} margin={{ left: 0, right: 8, top: 4 }}>
              {body}
              {keys.map((key) => (
                <Bar
                  key={key}
                  dataKey={key}
                  stackId={kind === "stacked" ? "one" : undefined}
                  fill={`var(--color-${key})`}
                  radius={2}
                />
              ))}
            </BarChart>
          )}
        </ChartContainer>
      ) : (
        <div className="h-48 w-full" aria-hidden="true" />
      )}
      {children}
    </section>
  );
}

/** A split's value in words: its own, none, or those the server summed. */
function labelOf(value: GroupValue): string {
  return value.kind === "value" ? value.value : value.kind === "none" ? "None" : "Other values";
}

/** Spend split by a group over time, its values that spent most each a
 *  colour, and every value's totals under it, which also say what the
 *  colours cannot. A user's or a session's row opens its runs. */
function SplitChart({
  chart,
  split,
  askedDays,
  hourly,
  organization,
  slug,
}: {
  chart: DashboardChart;
  split: Split;
  askedDays: number;
  hourly: boolean;
  organization: string;
  slug: string;
}) {
  const title = chartTitle(chart);
  if (split.kind !== "ok") {
    return (
      <section aria-label={title} className="grid gap-gutter rounded-md border border-border p-gutter lg:col-span-2">
        <h2 className="text-base font-medium">{title}</h2>
        <p className="text-sm text-muted-foreground">This split could not be read. Reload to try again.</p>
      </section>
    );
  }
  const { series, bars } = spendSeries(split.points, labelOf);
  const config: ChartConfig = Object.fromEntries(
    series.map((each) => [each.key, { label: each.label, color: each.color }]),
  );
  const totals = groupTotals(split.points);
  const href = (value: GroupValue): string | undefined => {
    if (value.kind !== "value") return undefined;
    if (chart.group === MetricsGroup.User) {
      return tracesHref(organization, slug, { user: [value.value], days: split.days });
    }
    if (chart.group === MetricsGroup.Session) {
      return sessionRunsHref(organization, slug, value.value, split.days);
    }
    return undefined;
  };
  return (
    <Chart
      title={title}
      summary={`${formatCost(totals.reduce((sum, total) => sum + total.spend, 0))} over the window, across ${
        totals.length
      } rows below.`}
      config={config}
      bars={bars}
      hourly={hourly}
      format={formatCost}
      kind="stacked"
      wide
    >
      {split.days < askedDays && (
        <p className="text-sm text-muted-foreground">
          The last {split.days === 1 ? "day" : `${split.days} days`}: a split by tag, user or session reads the spans,
          which this plan keeps no longer.
        </p>
      )}
      {totals.length === 0 ? (
        <p className="text-sm text-muted-foreground">No model call landed in the window.</p>
      ) : (
        <Rows>
          {totals.map((total) => {
            const to = href(total.value);
            const label = labelOf(total.value);
            return (
              <Row
                key={total.value.kind === "value" ? `=${total.value.value}` : total.value.kind}
                keep
                label={
                  to ? (
                    <Link href={to} className="truncate hover:underline">
                      {label}
                    </Link>
                  ) : total.value.kind === "value" ? (
                    <span className="truncate">{label}</span>
                  ) : (
                    <span className="text-muted-foreground">{label}</span>
                  )
                }
                hint={`${formatCount(total.tokens)} tokens · ${formatCount(total.runs)} runs${
                  total.failedRuns > 0 ? `, ${formatCount(total.failedRuns)} failed` : ""
                }`}
              >
                <span className="tabular-nums">{formatCost(total.spend)}</span>
              </Row>
            );
          })}
        </Rows>
      )}
    </Chart>
  );
}

/** A dashboard's charts over the window, in its order, two to a row; a
 *  split across both. */
export function DashboardCharts({
  organization,
  slug,
  charts,
  days,
  bars,
  firstToken,
  hourly,
  firstTokenHourly,
  splits,
}: {
  organization: string;
  slug: string;
  charts: readonly DashboardChart[];
  /** The window asked for, which a split from the spans may read less of. */
  days: number;
  bars: DashboardBar[];
  firstToken: FirstTokenBar[];
  hourly: boolean;
  /** Whether the spans' bars, which the histogram sizes to the window, are under a day. */
  firstTokenHourly: boolean;
  /** Each split chart's read, by its chart's word. */
  splits: Record<string, Split>;
}) {
  const sum = (of: (bar: DashboardBar) => number) => bars.reduce((total, bar) => total + of(bar), 0);
  const last = <T,>(rows: readonly T[], of: (row: T) => number | null) =>
    rows.findLast((row) => of(row) !== null) ?? null;
  const lastly = (value: number | null | undefined) =>
    value == null ? "None read in the window." : `Latest 95th percentile ${formatDuration(value)}.`;
  const drawn = (chart: DashboardChart) => {
    if (chart.group) {
      return (
        <SplitChart
          key={chartWord(chart)}
          chart={chart}
          split={splits[chartWord(chart)] ?? { kind: "unread" }}
          askedDays={days}
          hourly={hourly}
          organization={organization}
          slug={slug}
        />
      );
    }
    switch (chart.measure) {
      case ChartMeasure.Spend:
        return (
          <Chart
            key={chart.measure}
            title="Spend"
            summary={`${formatCost(sum((bar) => bar.spend))} over the window.`}
            config={SPEND}
            bars={bars}
            hourly={hourly}
            format={formatCost}
            kind="bars"
          />
        );
      case ChartMeasure.Tokens:
        return (
          <Chart
            key={chart.measure}
            title="Tokens"
            summary={`${formatCount(sum((bar) => bar.input))} input and ${formatCount(
              sum((bar) => bar.output),
            )} output over the window.`}
            config={TOKENS}
            bars={bars}
            hourly={hourly}
            format={formatCount}
            kind="stacked"
          />
        );
      case ChartMeasure.Runs:
        return (
          <Chart
            key={chart.measure}
            title="Runs"
            summary={`${formatCount(sum((bar) => bar.ok + bar.failed))} runs, ${formatCount(
              sum((bar) => bar.failed),
            )} failed, over the window.`}
            config={RUNS}
            bars={bars}
            hourly={hourly}
            format={formatCount}
            kind="stacked"
          />
        );
      case ChartMeasure.Latency:
        return (
          <Chart
            key={chart.measure}
            title="Latency"
            summary={lastly(last(bars, (bar) => bar.p95)?.p95)}
            config={LATENCY}
            bars={bars}
            hourly={hourly}
            format={formatDuration}
            kind="lines"
          />
        );
      case ChartMeasure.FirstToken:
        return (
          <Chart
            key={chart.measure}
            title="Time to first token"
            summary={lastly(last(firstToken, (bar) => bar.firstTokenP95)?.firstTokenP95)}
            config={FIRST_TOKEN}
            bars={firstToken}
            hourly={firstTokenHourly}
            format={formatDuration}
            kind="lines"
          />
        );
    }
  };
  return <div className="grid gap-page lg:grid-cols-2">{charts.map(drawn)}</div>;
}
