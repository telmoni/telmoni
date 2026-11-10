"use client";

import { useId } from "react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { TokenChart } from "@/components/traces/charts";
import type { Bar as DashboardBar } from "@/lib/dashboards";
import { useInBrowser } from "@/lib/in-browser";
import { formatCost, formatCount, formatDuration, type TokenBar } from "@/lib/traces";

/** One bar of time to first token, which only the spans keep. */
export type FirstTokenBar = {
  at: string;
  firstTokenP50: number | null;
  firstTokenP95: number | null;
};

// The theme's own colours, as the trace list's charts take them.
const SPEND: ChartConfig = { spend: { label: "Spend", color: "var(--muted-foreground)" } };
const TRACES: ChartConfig = {
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
}: {
  title: string;
  /** What the chart shows, in words, for a reader who cannot see it. */
  summary: string;
  config: ChartConfig;
  bars: readonly object[];
  hourly: boolean;
  format: (value: number) => string;
  kind: "bars" | "stacked" | "lines";
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
    <section aria-labelledby={heading} className="grid gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border">
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
    </section>
  );
}

/** The window's charts, two to a row. */
export function DashboardCharts({
  bars,
  firstToken,
  hourly,
  firstTokenHourly,
  tokenBars,
}: {
  bars: DashboardBar[];
  /** The tokens by model, or under one name where the grouped totals could not be read whole. */
  tokenBars: TokenBar[];
  firstToken: FirstTokenBar[];
  hourly: boolean;
  /** Whether the spans' bars, which the histogram sizes to the window, are under a day. */
  firstTokenHourly: boolean;
}) {
  const tokensId = useId();
  const sum = (of: (bar: DashboardBar) => number) => bars.reduce((total, bar) => total + of(bar), 0);
  const last = <T,>(rows: readonly T[], of: (row: T) => number | null) =>
    rows.findLast((row) => of(row) !== null) ?? null;
  const lastly = (value: number | null | undefined) =>
    value == null ? "None read in the window." : `Latest 95th percentile ${formatDuration(value)}.`;
  return (
    <div className="grid gap-page lg:grid-cols-2">
      <Chart
        title="Spend"
        summary={`${formatCost(sum((bar) => bar.spend))} over the window.`}
        config={SPEND}
        bars={bars}
        hourly={hourly}
        format={formatCost}
        kind="bars"
      />
      <section
        aria-labelledby={tokensId}
        className="grid min-w-0 gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border"
      >
        <h2 id={tokensId} className="text-base font-medium">
          Tokens
        </h2>
        {/* Any window past a day draws by the UTC day, whatever its length. */}
        <TokenChart bars={tokenBars} days={hourly ? 1 : 2} timeZone="UTC" className="h-48" />
      </section>
      <Chart
        title="Traces"
        summary={`${formatCount(sum((bar) => bar.ok + bar.failed))} traces, ${formatCount(
          sum((bar) => bar.failed),
        )} failed, over the window.`}
        config={TRACES}
        bars={bars}
        hourly={hourly}
        format={formatCount}
        kind="stacked"
      />
      <Chart
        title="Latency"
        summary={lastly(last(bars, (bar) => bar.p95)?.p95)}
        config={LATENCY}
        bars={bars}
        hourly={hourly}
        format={formatDuration}
        kind="lines"
      />
      <Chart
        title="Time to first token"
        summary={lastly(last(firstToken, (bar) => bar.firstTokenP95)?.firstTokenP95)}
        config={FIRST_TOKEN}
        bars={firstToken}
        hourly={firstTokenHourly}
        format={formatDuration}
        kind="lines"
      />
    </div>
  );
}
