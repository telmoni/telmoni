"use client";

import { useId, useState, useSyncExternalStore, type ReactNode } from "react";
import { ArrowUp, ArrowDown, GripVertical, PanelsTopLeft } from "lucide-react";
import { Bar, BarChart, CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";

import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { OutputSpeedChart, TokenChart } from "@/components/traces/charts";
import type { TraceHistogram } from "@/lib/server/entities/traces";
import { DEFAULT_SPEND_LAYOUT, SPEND_CHART_LABELS, readSpendLayout, type SpendLayout, type Bar as DashboardBar } from "@/lib/dashboards";
import { useInBrowser } from "@/lib/in-browser";
import { formatCost, formatCount, formatDuration, type TokenBar } from "@/lib/traces";
import { listeners } from "@/lib/listeners";
import { useUser } from "@/lib/store";
import { ChartMeasure } from "@/lib/types/enums";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

const LAYOUT_KEY = "telmoni-spend-layout:";
let cachedLayout: { key: string; stored: string | null; layout: SpendLayout } | null = null;
function onLayoutStorage(event: StorageEvent) {
  if (event.key === null || event.key.startsWith(LAYOUT_KEY)) layoutStore.emit();
}
const layoutStore = listeners({
  first: () => window.addEventListener("storage", onLayoutStorage),
  last: () => window.removeEventListener("storage", onLayoutStorage),
});
function savedLayout(key: string | null): SpendLayout {
  if (!key) return DEFAULT_SPEND_LAYOUT;
  let stored: string | null;
  try { stored = localStorage.getItem(key); } catch { return DEFAULT_SPEND_LAYOUT; }
  if (cachedLayout?.key === key && cachedLayout.stored === stored) return cachedLayout.layout;
  const layout = readSpendLayout(stored);
  cachedLayout = { key, stored, layout };
  return layout;
}

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

/** Personal chart order, visibility and width; editing does not change the data. */
export function DashboardCharts({
  bars,
  firstToken,
  hourly,
  firstTokenHourly,
  tokenBars,
  histogram,
}: {
  bars: DashboardBar[];
  /** The tokens by model, or under one name where the grouped totals could not be read whole. */
  tokenBars: TokenBar[];
  firstToken: FirstTokenBar[];
  hourly: boolean;
  /** Whether the spans' bars, which the histogram sizes to the window, are under a day. */
  firstTokenHourly: boolean;
  /** The spans' histogram, which output speed is read from; null where it could not be read. */
  histogram: TraceHistogram | null;
}) {
  const tokensId = useId();
  const speedId = useId();
  const owner = useUser()?.id ?? null;
  const storageKey = owner ? `${LAYOUT_KEY}${owner}` : null;
  const saved = useSyncExternalStore(layoutStore.subscribe, () => savedLayout(storageKey), () => DEFAULT_SPEND_LAYOUT);
  const [draft, setDraft] = useState<{ owner: string; layout: SpendLayout } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragged, setDragged] = useState<ChartMeasure | null>(null);
  const editing = draft?.owner === owner && draft !== null;
  const layout = editing ? draft.layout : saved;
  function change(next: SpendLayout) {
    if (owner) setDraft({ owner, layout: next });
    setError(null);
  }
  function move(from: number, to: number) {
    if (from === to || from < 0 || to < 0 || to >= layout.length) return;
    const next = [...layout];
    const [item] = next.splice(from, 1);
    if (!item) return;
    next.splice(to, 0, item);
    change(next);
  }
  function save() {
    if (!storageKey) return;
    try {
      localStorage.setItem(storageKey, JSON.stringify(layout));
      layoutStore.emit();
      setDraft(null);
      setError(null);
    } catch {
      setError("Your browser could not save this layout. Allow site storage, then try again.");
    }
  }
  const sum = (of: (bar: DashboardBar) => number) => bars.reduce((total, bar) => total + of(bar), 0);
  const last = <T,>(rows: readonly T[], of: (row: T) => number | null) =>
    rows.findLast((row) => of(row) !== null) ?? null;
  const lastly = (value: number | null | undefined) =>
    value == null ? "None read in the window." : `Latest 95th percentile ${formatDuration(value)}.`;
  const charts: Record<ChartMeasure, ReactNode> = {
    spend: <Chart
        title="Spend"
        summary={`${formatCost(sum((bar) => bar.spend))} over the window.`}
        config={SPEND}
        bars={bars}
        hourly={hourly}
        format={formatCost}
        kind="bars"
      />,
    tokens: <section
        aria-labelledby={tokensId}
        className="grid min-w-0 gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border"
      >
        <h2 id={tokensId} className="text-base font-medium">
          Tokens
        </h2>
        {/* Any window past a day draws by the UTC day, whatever its length. */}
        <TokenChart bars={tokenBars} days={hourly ? 1 : 2} timeZone="UTC" className="h-48" />
      </section>,
    traces: <Chart
        title="Traces"
        summary={`${formatCount(sum((bar) => bar.ok + bar.failed))} traces, ${formatCount(
          sum((bar) => bar.failed),
        )} failed, over the window.`}
        config={TRACES}
        bars={bars}
        hourly={hourly}
        format={formatCount}
        kind="stacked"
      />,
    latency: <Chart
        title="Latency"
        summary={lastly(last(bars, (bar) => bar.p95)?.p95)}
        config={LATENCY}
        bars={bars}
        hourly={hourly}
        format={formatDuration}
        kind="lines"
      />,
    first_token: <Chart
        title="Time to first token"
        summary={lastly(last(firstToken, (bar) => bar.firstTokenP95)?.firstTokenP95)}
        config={FIRST_TOKEN}
        bars={firstToken}
        hourly={firstTokenHourly}
        format={formatDuration}
        kind="lines"
      />,
    output_speed: <section
        aria-labelledby={speedId}
        className="grid min-w-0 gap-gutter rounded-menu p-gutter ring-[0.5px] ring-border"
      >
        <h2 id={speedId} className="text-base font-medium">
          Output speed
        </h2>
        {histogram ? (
          <>
            <p className="text-muted-foreground">
              Model calls over the last {histogram.days === 1 ? "day" : `${histogram.days} days`}, as far
              back as the project keeps its spans.
            </p>
            <OutputSpeedChart
              bars={histogram.bars}
              days={histogram.days}
              bucketSeconds={histogram.bucket_seconds}
              timeZone="UTC"
              className="h-48"
            />
          </>
        ) : (
          <p className="text-muted-foreground">Output speed could not be read. Reload to try again.</p>
        )}
      </section>,
  };
  return (
    <div className="grid gap-gutter">
      <div className="flex flex-wrap items-center justify-end gap-gutter">
        {editing ? (
          <>
            <p className="mr-auto text-sm text-muted-foreground">Drag charts or use the arrows. Saved for you in this browser.</p>
            <Button onClick={() => change(DEFAULT_SPEND_LAYOUT)}>Reset layout</Button>
            <Button onClick={() => { setDraft(null); setError(null); }}>Cancel</Button>
            <Button variant="filled" onClick={save}>Save layout</Button>
          </>
        ) : (
          <Button disabled={!owner} onClick={() => { if (owner) setDraft({ owner, layout: saved }); setError(null); }}>
            <PanelsTopLeft aria-hidden="true" /> Edit layout
          </Button>
        )}
      </div>
      {editing && error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {!editing && !layout.some((row) => row.visible) && (
        <p className="text-sm text-muted-foreground">All charts are hidden. Use Edit layout to show them again.</p>
      )}
      <div className="grid items-start gap-page lg:grid-cols-2">
        {layout.map((row, index) => (editing || row.visible) && (
          <div
            key={row.measure}
            className={cn("grid min-w-0 gap-gutter", row.width === "full" && "lg:col-span-2", editing && "rounded-menu border border-dashed border-border p-gutter")}
            onDragOver={(event) => { if (editing && dragged) event.preventDefault(); }}
            onDrop={(event) => {
              if (!editing || !dragged) return;
              event.preventDefault();
              move(layout.findIndex((item) => item.measure === dragged), index);
              setDragged(null);
            }}
          >
            {editing && (
              <div className="flex flex-wrap items-center gap-gutter">
                <Button
                  size="icon"
                  aria-label={`Drag ${SPEND_CHART_LABELS[row.measure]}`}
                  draggable
                  onDragStart={(event) => {
                    event.dataTransfer.setData("text/plain", row.measure);
                    event.dataTransfer.effectAllowed = "move";
                    setDragged(row.measure);
                  }}
                  onDragEnd={() => setDragged(null)}
                  className="cursor-grab active:cursor-grabbing"
                ><GripVertical aria-hidden="true" /></Button>
                <Button size="icon" aria-label={`Move ${SPEND_CHART_LABELS[row.measure]} earlier`} disabled={index === 0} onClick={() => move(index, index - 1)}><ArrowUp aria-hidden="true" /></Button>
                <Button size="icon" aria-label={`Move ${SPEND_CHART_LABELS[row.measure]} later`} disabled={index === layout.length - 1} onClick={() => move(index, index + 1)}><ArrowDown aria-hidden="true" /></Button>
                <label className="flex items-center gap-gutter text-sm">
                  <Checkbox checked={row.visible} onCheckedChange={(checked) => change(layout.map((item, at) => at === index ? { ...item, visible: checked === true } : item))} />
                  Show {SPEND_CHART_LABELS[row.measure]}
                </label>
                <Button className="ml-auto" aria-label={`Make ${SPEND_CHART_LABELS[row.measure]} ${row.width === "half" ? "full" : "half"} width`} onClick={() => change(layout.map((item, at) => at === index ? { ...item, width: item.width === "half" ? "full" : "half" } : item))}>
                  {row.width === "half" ? "Half width" : "Full width"}
                </Button>
              </div>
            )}
            {row.visible ? charts[row.measure] : <p className="text-sm text-muted-foreground">{SPEND_CHART_LABELS[row.measure]} is hidden.</p>}
          </div>
        ))}
      </div>
    </div>
  );
}
