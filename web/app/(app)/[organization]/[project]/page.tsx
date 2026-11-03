import type { Metadata } from "next";
import Link from "next/link";
import {
  Activity,
  BellRing,
  BookOpen,
  FolderOpen,
  Inbox,
  PiggyBank,
} from "lucide-react";

import { CopyButton } from "@/components/copy-button";
import { LocalTime } from "@/components/local-time";
import { MarkRead } from "@/components/mark-read";
import { Figure, OverviewCard, OverviewGrid } from "@/components/overview-card";
import { RoleBadge } from "@/components/role-badge";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { SEVERITY_LABEL, incidentHref, severityVariant } from "@/lib/alerts";
import { CONTENT_MODE_LABEL } from "@/lib/content-mode";
import { totalsOf } from "@/lib/overview";
import { fetchContentMode, fetchProjectBySlug, fetchProjectNotifications } from "@/lib/server/data";
import { fetchAlerts } from "@/lib/server/entities/alerts";
import { fetchRollups } from "@/lib/server/entities/dashboards";
import { DOCS_URL } from "@/lib/site";
import { projectPath } from "@/lib/slug";
import { formatCost, formatCount } from "@/lib/traces";
import { IncidentStatus, RuleAggregation, RuleMeasure, RuleWindow, SpanKind } from "@/lib/types/enums";

import { Notices } from "../../_notices";
import { Overview } from "../../_overview";
import { markProjectReadAction } from "./notice-actions";

// The tab is titled as the page is, by the project's name.
export async function generateMetadata({
  params,
}: PageProps<"/[organization]/[project]">): Promise<Metadata> {
  const { project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  return { title: project ? project.name : "Overview" };
}

// The project at a glance, a card a subject, each leading to the page that holds it: what
// it is, its last day, what is open, its budgets, its notices and the book. Anybody on the project reads every card.
export default async function OverviewPage({
  params,
}: PageProps<"/[organization]/[project]">) {
  const { organization, project: slug } = await params;
  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;
  const base = projectPath(organization, project.slug);

  const [notices, day, alerts, mode] = await Promise.all([
    fetchProjectNotifications(project.id),
    fetchRollups(project.id, 1, null),
    fetchAlerts(project.id),
    fetchContentMode(project.id),
  ]);

  const totals = day.kind === "ok" ? totalsOf(day.rollups.points) : null;
  const open =
    alerts.kind === "ok" ? alerts.incidents.filter((i) => i.status !== IncidentStatus.Resolved) : null;
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
  // Mark read goes up to the newest notice drawn, the first of the feed.
  const newest = notices?.items[0];
  const unreadable = <p className="text-muted-foreground">Could not be read just now. Reload to try again.</p>;

  return (
    <Overview
      title={project.name}
      sections={
        <OverviewGrid>
          <OverviewCard
            title="Project info"
            icon={FolderOpen}
            links={[
              { href: `${base}/members`, label: "Add people to this project" },
              { href: `${base}/settings`, label: "Go to project settings" },
            ]}
          >
            <dl className="grid gap-gutter">
              <div>
                <dt className="text-muted-foreground">Project name</dt>
                <dd>{project.name}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Project ID</dt>
                <dd className="flex items-center gap-gutter">
                  <code className="min-w-0 truncate font-mono text-xs">{project.id}</code>
                  <CopyButton value={project.id} label="the project ID" />
                </dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Content</dt>
                <dd>{mode ? CONTENT_MODE_LABEL[mode.content_mode] : "Not read"}</dd>
              </div>
              <div>
                <dt className="text-muted-foreground">Your role</dt>
                <dd>
                  <RoleBadge role={project.role} />
                </dd>
              </div>
            </dl>
          </OverviewCard>

          <OverviewCard
            title="Last 24 hours"
            icon={Activity}
            links={[{ href: `${base}/dashboards?days=1`, label: "Go to Dashboards" }]}
          >
            {totals === null ? (
              unreadable
            ) : (
              <div className="grid grid-cols-2 gap-gutter">
                <Figure value={formatCount(totals.runs)} label="Runs" />
                <Figure value={formatCount(totals.failedRuns)} label="Failed runs" />
                <Figure value={formatCost(totals.spend)} label="Spend" />
                <Figure value={formatCount(totals.tokens)} label="Tokens" />
              </div>
            )}
          </OverviewCard>

          <OverviewCard
            title="Open incidents"
            icon={BellRing}
            links={[{ href: `${base}/alerts`, label: "Go to Alerts" }]}
          >
            {open === null ? (
              unreadable
            ) : open.length === 0 ? (
              <p className="text-muted-foreground">None open: every alert holds.</p>
            ) : (
              <ul className="grid gap-gutter">
                {open.slice(0, 5).map((incident) => (
                  <li key={incident.id} className="flex items-start justify-between gap-gutter">
                    <span className="min-w-0">
                      <Link
                        href={incidentHref(organization, project.slug, incident.id)}
                        className="block truncate underline-offset-4 hover:underline"
                      >
                        {incident.rule_name}
                      </Link>
                      <span className="text-xs text-muted-foreground">
                        Opened <LocalTime iso={incident.opened_at} />
                      </span>
                    </span>
                    <Badge variant={severityVariant(incident.severity)} className="font-normal">
                      {SEVERITY_LABEL[incident.severity]}
                    </Badge>
                  </li>
                ))}
              </ul>
            )}
          </OverviewCard>

          <OverviewCard
            title="Budgets this month"
            icon={PiggyBank}
            links={[{ href: `${base}/alerts`, label: budgets.length > 0 ? "Go to Alerts" : "Set a budget" }]}
          >
            {alerts.kind !== "ok" ? (
              unreadable
            ) : budgets.length === 0 ? (
              <p className="text-muted-foreground">
                None set. A budget is a cost alert summed over the month.
              </p>
            ) : (
              <ul className="grid gap-gutter">
                {budgets.map((budget) => (
                  <li key={budget.id} className="flex items-baseline justify-between gap-gutter">
                    <span className="min-w-0 truncate">{budget.name}</span>
                    <span className="shrink-0 tabular-nums text-muted-foreground">
                      {budget.value === null ? "Not read yet" : formatCost(budget.value)} of{" "}
                      {formatCost(budget.alert_threshold)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </OverviewCard>

          <Notices
            label="Project notices"
            icon={Inbox}
            feed={notices}
            project={{ organization, slug }}
            action={
              notices && newest && notices.unread > 0 ? (
                <MarkRead
                  unread={notices.unread}
                  mark={markProjectReadAction.bind(null, project.id, newest.created_at)}
                />
              ) : undefined
            }
          />

          <OverviewCard
            title="Documentation"
            icon={BookOpen}
            links={[{ href: DOCS_URL, label: "Read the docs" }]}
          >
            <ul className="grid gap-gutter">
              {[
                { href: `${DOCS_URL}/api/sdks`, label: "Send runs with an SDK" },
                { href: `${DOCS_URL}/api/frameworks`, label: "Send from a framework you use" },
                { href: `${DOCS_URL}/monitoring/alerts`, label: "Alerts and budgets" },
                { href: `${DOCS_URL}/monitoring/content`, label: "What content is kept" },
              ].map((doc) => (
                <li key={doc.href}>
                  <Link href={doc.href} className="underline-offset-4 hover:underline">
                    {doc.label}
                  </Link>
                </li>
              ))}
            </ul>
          </OverviewCard>
        </OverviewGrid>
      }
    />
  );
}
