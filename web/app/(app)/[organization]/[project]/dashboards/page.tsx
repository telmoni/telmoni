import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { WindowPicker } from "@/components/window-picker";
import { COST_DASHBOARD, barsOf, chartWord, chartsOf, type Chart } from "@/lib/dashboards";
import type { DashboardView } from "@/lib/server/entities/traces";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { fetchDashboards, fetchRollups } from "@/lib/server/entities/dashboards";
import { fetchTraceFacets, fetchTraceHistogram } from "@/lib/server/entities/traces";
import { projectPage } from "@/lib/server/project-page";
import { MAX_DAYS, OPENING_DAYS, TOTALS_WINDOWS } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import { formatCost, parseTraceQuery, tagKeys } from "@/lib/traces";
import { ChartMeasure, RuleAggregation, RuleMeasure, RuleWindow, SpanKind } from "@/lib/types/enums";

import { DashboardCharts, type Split } from "./_charts";
import { ChartsPicker } from "./_charts-picker";
import { DashboardsMenu, type SavedDashboard } from "./_dashboards-menu";

export const metadata = { title: "Spend" };

/** The window the URL asks for, the opening week where it asks for none. */
function daysOf(given: string | string[] | undefined): number {
  const days = Number(Array.isArray(given) ? given[0] : given);
  return Number.isInteger(days) && days >= 1 && days <= MAX_DAYS ? days : OPENING_DAYS;
}

/** A saved dashboard's charts as the URL's are read, any the page no longer
 *  draws left out. */
function savedOf(dashboard: DashboardView): SavedDashboard {
  return {
    id: dashboard.id,
    name: dashboard.name,
    charts: chartsOf(dashboard.query.charts.map((chart) => chartWord(chart))) ?? [],
  };
}

// A dashboard's charts over the project's hourly totals — spend, tokens, runs
// and their failures, latency, time to first token from the spans, and spend
// split by a group, a tag, a user or a session — under the month's budgets:
// the URL's charts, else the saved dashboard it names, else the cost
// dashboard. Owners and admins save one for everyone on the project.
export default async function DashboardsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/dashboards">) {
  const { organization, project: slug } = await params;
  const query = await searchParams;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const days = daysOf(query.days);
  const window = parseTraceQuery({ days: String(days) });

  const [totals, alerts, saved, facets] = await Promise.all([
    fetchRollups(project.id, days),
    fetchAlerts(project.id),
    fetchDashboards(project.id),
    fetchTraceFacets(project.id, window),
  ]);
  const dashboards = saved.kind === "ok" ? saved.dashboards.map(savedOf) : null;
  const viewId = Array.isArray(query.view) ? query.view[0] : query.view;
  const current = dashboards?.find((dashboard) => dashboard.id === viewId);
  const charts: Chart[] = chartsOf(query.chart) ?? (current?.charts.length ? current.charts : [...COST_DASHBOARD]);

  const split = charts.filter((chart) => chart.group);
  const [histogram, ...reads] = await Promise.all([
    charts.some((chart) => chart.measure === ChartMeasure.FirstToken) ? fetchTraceHistogram(project.id, window, 0) : null,
    ...split.map((chart) => fetchRollups(project.id, days, chart.group, chart.tag)),
  ]);
  const splits: Record<string, Split> = Object.fromEntries(
    split.map((chart, i) => {
      const read = reads[i];
      return [
        chartWord(chart),
        read?.kind === "ok" ? { kind: "ok", points: read.rollups.points, days: read.rollups.days } : { kind: "unread" },
      ];
    }),
  );
  const truncated =
    (totals.kind === "ok" && totals.rollups.truncated) || reads.some((read) => read.kind === "ok" && read.rollups.truncated);
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
        title="Spend"
        info="What the project's agents spend, against this month's budgets, and a dashboard's charts over the window, by the UTC day or, for a day, the hour: spend, tokens, runs, latency and time to first token from the hourly totals and, for time to first token, the spans; spend split by a model, an agent, a provider, an environment or a release from the totals, or by a tag, a user or a session from the spans the plan keeps, the ten that spent most each and the rest as one. Spend is priced as it is read, the project's own prices first. Owners and admins save a dashboard for everyone on the project."
        action={
          <div className="flex flex-wrap items-center gap-gutter">
            <DashboardsMenu
              organization={organization}
              slug={slug}
              projectId={project.id}
              days={days}
              charts={charts}
              dashboards={dashboards}
              current={current}
              canSave={administers(project.role)}
            />
            <ChartsPicker
              charts={charts}
              tagKeys={facets.kind === "ok" ? tagKeys(facets.facets.tag.values.map((each) => each.value)) : []}
            />
            <WindowPicker days={days} windows={TOTALS_WINDOWS} />
          </div>
        }
      />
      <div className="grid gap-page">
        {budgets.length > 0 && (
          <section aria-labelledby="budgets" className="grid gap-gutter">
            <h2 id="budgets" className="text-base font-medium">
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
            organization={organization}
            slug={slug}
            charts={charts}
            days={days}
            bars={barsOf(totals.rollups.points)}
            firstToken={
              histogram?.kind === "ok"
                ? histogram.histogram.bars.map((bar) => ({
                    at: bar.at,
                    firstTokenP50: bar.time_to_first_token_p50_ms,
                    firstTokenP95: bar.time_to_first_token_p95_ms,
                  }))
                : []
            }
            hourly={totals.rollups.step === "hour"}
            firstTokenHourly={histogram?.kind === "ok" && histogram.histogram.bucket_seconds < 86_400}
            splits={splits}
          />
        )}
        {truncated && (
          <NoticeCard
            title="The window holds more than one read answers."
            body="Its oldest days are left out. Choose a shorter window to see them all."
          />
        )}
      </div>
    </>
  );
}

