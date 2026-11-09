import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { WindowPicker } from "@/components/window-picker";
import { barsOf } from "@/lib/dashboards";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { fetchRollups } from "@/lib/server/entities/dashboards";
import { fetchTraceHistogram } from "@/lib/server/entities/traces";
import { projectPage } from "@/lib/server/project-page";
import { MAX_DAYS, OPENING_DAYS, TOTALS_WINDOWS } from "@/lib/list-query";
import { formatCost, parseTraceQuery } from "@/lib/traces";
import { RuleAggregation, RuleMeasure, RuleWindow, SpanKind } from "@/lib/types/enums";

import { DashboardCharts } from "./_charts";

export const metadata = { title: "Spend" };

/** The window the URL asks for, the opening week where it asks for none. */
function daysOf(given: string | string[] | undefined): number {
  const days = Number(Array.isArray(given) ? given[0] : given);
  return Number.isInteger(days) && days >= 1 && days <= MAX_DAYS ? days : OPENING_DAYS;
}

// The month's budgets, then the window's charts: spend, tokens, runs and
// latency from the hourly totals, time to first token from the spans.
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
  const window = parseTraceQuery({ days: String(days) });

  const [totals, alerts, histogram] = await Promise.all([
    fetchRollups(project.id, days),
    fetchAlerts(project.id),
    fetchTraceHistogram(project.id, window, 0),
  ]);
  const truncated = totals.kind === "ok" && totals.rollups.truncated;
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
        info="What the project's agents spend, against this month's budgets, and charts over the window, by the UTC day or, for a day, the hour: spend, tokens, runs and latency from the hourly totals, and time to first token from the spans. Spend is priced as it is read, the project's own prices first."
        action={<WindowPicker days={days} windows={TOTALS_WINDOWS} />}
      />
      <div className="grid gap-page">
        {budgets.length > 0 && (
          // A region named by its heading, which `Section` does not make.
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
      </div>
    </>
  );
}

