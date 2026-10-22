"use client";

import { useSyncExternalStore } from "react";
import { Bar, BarChart, Cell, ReferenceLine, XAxis, YAxis } from "recharts";

import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { openPeek } from "@/lib/peek";
import { STATUS_LABEL, formatDuration } from "@/lib/traces";
import { SpanStatus } from "@/lib/types/enums";

// The theme's colours, as the trace list's runs chart has them: a run that
// ended well in the muted ink, a failure in the destructive red, a
// cancelled one fainter than either.
const CONFIG: ChartConfig = {
  [SpanStatus.Ok]: { label: STATUS_LABEL[SpanStatus.Ok], color: "var(--muted-foreground)" },
  [SpanStatus.Error]: { label: STATUS_LABEL[SpanStatus.Error], color: "var(--destructive)" },
  [SpanStatus.Cancelled]: { label: STATUS_LABEL[SpanStatus.Cancelled], color: "var(--ring)" },
};

/** One run as the strip draws it. */
export type StripRun = {
  run_id: string;
  started_at: string;
  duration_ms: number;
  status: SpanStatus;
};

const subscribeNever = () => () => {};

// A monitor's last runs, a bar each, oldest at the left: as tall as the run
// was long and coloured as it ended, the expected duration a line across.
// Never an uptime grid nor a percentage. A bar opens its trace beside the
// page, as a row of the table under it does.
export function RunStrip({
  runs,
  expectedMs,
}: {
  runs: readonly StripRun[];
  expectedMs: number | null;
}) {
  // A run's time is written in the reader's clock, which the server does
  // not know: a box of the chart's height stands in until the browser draws.
  const inBrowser = useSyncExternalStore(
    subscribeNever,
    () => true,
    () => false,
  );
  const failed = runs.filter((r) => r.status === SpanStatus.Error).length;
  const summary = `${runs.length} ${runs.length === 1 ? "run" : "runs"}, ${failed} in error`;
  if (!inBrowser) return <div className="h-32 w-full" role="img" aria-label={summary} />;
  const data = [...runs]
    .sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at))
    .map((run) => ({ ...run, ms: run.duration_ms }));
  return (
    <ChartContainer
      config={CONFIG}
      className="aspect-auto h-32 w-full"
      role="img"
      aria-label={summary}
    >
      <BarChart data={data} margin={{ left: 0, right: 8, top: 4 }}>
        <XAxis dataKey="run_id" hide />
        <YAxis
          width="auto"
          tickLine={false}
          axisLine={false}
          tickFormatter={(ms: number) => formatDuration(ms)}
        />
        <ChartTooltip
          cursor={{ fillOpacity: 0.5 }}
          content={
            <ChartTooltipContent
              hideIndicator
              labelFormatter={(_, payload) => {
                const iso = payload?.[0]?.payload?.started_at;
                return typeof iso === "string" ? new Date(iso).toLocaleString() : null;
              }}
              formatter={(_, __, item) => {
                const run = item.payload as StripRun;
                return (
                  <span>
                    {STATUS_LABEL[run.status]} · {formatDuration(run.duration_ms)}
                  </span>
                );
              }}
            />
          }
        />
        {expectedMs !== null && (
          <ReferenceLine
            y={expectedMs}
            stroke="var(--foreground)"
            strokeDasharray="4 4"
            ifOverflow="extendDomain"
          />
        )}
        <Bar
          dataKey="ms"
          minPointSize={2}
          radius={2}
          className="cursor-pointer"
          onClick={(entry: { payload?: StripRun }) => {
            if (entry.payload) openPeek(entry.payload.run_id);
          }}
        >
          {data.map((run) => (
            <Cell key={run.run_id} fill={`var(--color-${run.status})`} />
          ))}
        </Bar>
      </BarChart>
    </ChartContainer>
  );
}
