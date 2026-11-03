import type { Chart, SpendGroup } from "@/lib/dashboards";
import { ChartMeasure, MetricsGroup } from "@/lib/types/enums";

// The words a chart is called by, in its picker and over it.

export const MEASURE_LABEL: Record<ChartMeasure, string> = {
  [ChartMeasure.Spend]: "Spend",
  [ChartMeasure.Tokens]: "Tokens",
  [ChartMeasure.Runs]: "Runs",
  [ChartMeasure.Latency]: "Latency",
  [ChartMeasure.FirstToken]: "Time to first token",
};

export const GROUP_LABEL: Record<SpendGroup, string> = {
  [MetricsGroup.Model]: "Model",
  [MetricsGroup.Agent]: "Agent",
  [MetricsGroup.Provider]: "Provider",
  [MetricsGroup.Environment]: "Environment",
  [MetricsGroup.Release]: "Release",
  [MetricsGroup.Tag]: "Tag",
  [MetricsGroup.User]: "User",
  [MetricsGroup.Session]: "Session",
};

/** A chart's title: its measure, and for a split what splits it. */
export function chartTitle(chart: Chart): string {
  if (!chart.group) return MEASURE_LABEL[chart.measure];
  return chart.group === MetricsGroup.Tag
    ? `Spend by tag ${chart.tag}`
    : `Spend by ${GROUP_LABEL[chart.group].toLowerCase()}`;
}
