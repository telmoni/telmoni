"use client";

import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import type { Bar as DashboardBar } from "@/lib/dashboards";
import { useInBrowser } from "@/lib/in-browser";
import { formatCost, formatCount, formatDuration } from "@/lib/traces";

/** One bar of time to first token, which only the spans keep. */
export type FirstTokenBar = {
  at: string;
  firstTokenP50: number | null;
  firstTokenP95: number | null;
};

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

/** An axis tick: the hour for a day's window, the date for a longer one. */
function tick(iso: string, hourly: boolean): string {
  const at = new Date(iso);
  return hourly
    ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : at.toLocaleDateString([], { month: "short", day: "numeric" });
}

function Chart({
  title,
  config,
  bars,
  hourly,
  format,
  kind,
}: {
  title: string;
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
  return (
    <section aria-label={title} className="grid gap-gutter rounded-md border border-border p-gutter">
      <h2 className="text-sm font-medium">{title}</h2>
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

/** The project's charts over the window, two to a row. */
export function DashboardCharts({
  bars,
  firstToken,
  hourly,
}: {
  bars: DashboardBar[];
  firstToken: FirstTokenBar[];
  hourly: boolean;
}) {
  return (
    <div className="grid gap-gutter lg:grid-cols-2">
      <Chart title="Spend" config={SPEND} bars={bars} hourly={hourly} format={formatCost} kind="bars" />
      <Chart title="Tokens" config={TOKENS} bars={bars} hourly={hourly} format={formatCount} kind="stacked" />
      <Chart title="Runs" config={RUNS} bars={bars} hourly={hourly} format={formatCount} kind="stacked" />
      <Chart title="Latency" config={LATENCY} bars={bars} hourly={hourly} format={formatDuration} kind="lines" />
      <Chart
        title="Time to first token"
        config={FIRST_TOKEN}
        bars={firstToken}
        hourly={hourly}
        format={formatDuration}
        kind="lines"
      />
    </div>
  );
}
