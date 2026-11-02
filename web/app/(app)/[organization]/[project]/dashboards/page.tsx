import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { WindowPicker } from "@/components/window-picker";
import { ROLLUP_GROUPS, barsOf, groupTotals, type RollupGroup } from "@/lib/dashboards";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { fetchRollups } from "@/lib/server/entities/dashboards";
import { fetchTraceHistogram } from "@/lib/server/entities/traces";
import { projectPage } from "@/lib/server/project-page";
import { OPENING_DAYS } from "@/lib/list-query";
import { formatCost, formatCount, parseTraceQuery } from "@/lib/traces";
import { RuleAggregation, RuleMeasure, RuleWindow, SpanKind } from "@/lib/types/enums";

import { DashboardCharts } from "./_charts";
import { GroupPicker } from "./_group-picker";

export const metadata = { title: "Dashboards" };

/** The window the URL asks for, the opening week where it asks for none. */
function daysOf(given: string | string[] | undefined): number {
  const days = Number(Array.isArray(given) ? given[0] : given);
  return Number.isInteger(days) && days >= 1 && days <= 360 ? days : OPENING_DAYS;
}

function groupOf(given: string | string[] | undefined): RollupGroup | null {
  const group = Array.isArray(given) ? given[0] : given;
  return (ROLLUP_GROUPS as readonly string[]).includes(group ?? "") ? (group as RollupGroup) : null;
}

// The project's charts over its hourly totals: spend, tokens, runs and their
// failures, latency, and time to first token, which the spans alone keep;
// the month's budgets above them, and each group's totals below.
export default async function DashboardsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/dashboards">) {
  const { project: slug } = await params;
  const query = await searchParams;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const days = daysOf(query.days);
  const group = groupOf(query.group);

  const [totals, grouped, histogram, alerts] = await Promise.all([
    fetchRollups(project.id, days, null),
    group ? fetchRollups(project.id, days, group) : null,
    fetchTraceHistogram(project.id, parseTraceQuery({ days: String(days) }), 0),
    fetchAlerts(project.id),
  ]);
  const truncated =
    (totals.kind === "ok" && totals.rollups.truncated) || (grouped?.kind === "ok" && grouped.rollups.truncated);
  const budgets =
    alerts.kind === "ok"
      ? alerts.alerts.filter(
          (alert) =>
            alert.kind === SpanKind.Run &&
            alert.measure === RuleMeasure.Cost &&
            alert.aggregation === RuleAggregation.Sum &&
            alert.window === RuleWindow.Month &&
            alert.paused === null,
        )
      : [];

  return (
    <>
      <PageHeader
        title="Dashboards"
        info="This project's spend, tokens, runs, latency and time to first token over the window, by the UTC day or, for a day, the hour, from the hourly totals and, for time to first token, the spans; spend priced as it is read, the project's own prices first."
        action={
          <div className="flex items-center gap-gutter">
            <GroupPicker group={group} />
            <WindowPicker days={days} />
          </div>
        }
      />
      <div className="grid gap-gutter">
        {budgets.length > 0 && (
          <section aria-labelledby="budgets" className="grid gap-gutter">
            <h2 id="budgets" className="text-sm font-medium">
              Budgets this month
            </h2>
            <Rows>
              {budgets.map((budget) => (
                <Row key={budget.id} keep label={budget.name}>
                  <span className="tabular-nums">
                    {budget.value === null ? "Not read yet" : formatCost(budget.value)} of{" "}
                    {formatCost(budget.alert_threshold)}
                  </span>
                </Row>
              ))}
            </Rows>
          </section>
        )}
        {totals.kind !== "ok" ? (
          <NoticeCard
            title="The totals could not be read."
            body="Reload to try again. The Traces page charts a window's runs, tokens and durations."
          />
        ) : totals.rollups.points.length === 0 ? (
          <NoticeCard
            title="Nothing to show yet."
            body={`No run has landed in the last ${
              totals.rollups.days === 1 ? "day" : `${totals.rollups.days} days`
            }.`}
          />
        ) : (
          <DashboardCharts
            bars={barsOf(totals.rollups.points)}
            firstToken={
              histogram.kind === "ok"
                ? histogram.histogram.bars.map((bar) => ({
                    at: bar.at,
                    firstTokenP50: bar.time_to_first_token_p50_ms,
                    firstTokenP95: bar.time_to_first_token_p95_ms,
                  }))
                : []
            }
            hourly={totals.rollups.step === "hour"}
            firstTokenHourly={histogram.kind === "ok" && histogram.histogram.bucket_seconds < 86_400}
          />
        )}
        {truncated && (
          <NoticeCard
            title="The window holds more than one read answers."
            body="Its oldest days are left out. Choose a shorter window to see them all."
          />
        )}
        {grouped?.kind === "ok" && grouped.rollups.points.length > 0 && (
          <section aria-labelledby="by-group" className="grid gap-gutter">
            <h2 id="by-group" className="text-sm font-medium">
              By {group}
            </h2>
            <Rows>
              {groupTotals(grouped.rollups.points).map((row) => (
                <Row
                  key={row.value}
                  keep
                  label={row.value || <span className="text-muted-foreground">None</span>}
                  hint={`${formatCount(row.tokens)} tokens · ${formatCount(row.runs)} runs${
                    row.failedRuns > 0 ? `, ${formatCount(row.failedRuns)} failed` : ""
                  }`}
                >
                  <span className="tabular-nums">{formatCost(row.spend)}</span>
                </Row>
              ))}
            </Rows>
          </section>
        )}
      </div>
    </>
  );
}
