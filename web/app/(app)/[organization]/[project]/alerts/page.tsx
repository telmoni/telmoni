import type { ReactNode } from "react";
import { ArrowUpRight } from "lucide-react";
import Link from "next/link";
import { z } from "zod";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import {
  ALERTS_HEADING,
  CONNECTION_STATUS_LABEL,
  LEVEL_LABEL,
  MISSING_DATA_LABEL,
  SEVERITY_LABEL,
  STATUS_LABEL,
  alertHeading,
  amount,
  connectionChoices,
  filterWords,
  hoursWords,
  inForce,
  metricWords,
  precisionWords,
  renotifyWords,
  severityVariant,
  thresholdWords,
  toldWords,
  windowsFor,
  type ConnectionChoice,
} from "@/lib/alerts";
import { ownerContact } from "@/lib/identity";
import {
  INCIDENTS_MAX,
  activeOrganization,
  fetchAlerts,
  fetchConnectors,
  fetchIncident,
  fetchMembers,
  fetchOrganizationMembers,
  fetchProjectBySlug,
  getServerContext,
  type Alert,
  type Incident,
  type Mute,
  type Precision,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { shortId, traceHref } from "@/lib/traces";
import { IncidentStatus, Severity } from "@/lib/types/enums";

import { AlertActions, HearAgain } from "./_alert-actions";
import { NewAlert } from "./_alert-panel";
import { IncidentActions } from "./_incident-actions";
import { EndMute, MuteButton } from "./_mute";

export const metadata = { title: "Alerts" };

// The project's alerts, built as Langfuse's are: the week's precision at the
// top; then its incidents, those not resolved first, each with its alert,
// its severity, when, and where its notices went, acknowledged or resolved
// by any member; then every alert with its metric, filters, window,
// thresholds, severity, connectors and maintenance windows, each made,
// edited, paused, muted and deleted here. A trace a notice showed links
// back with `?incident=`, which draws that incident even past the list.
export default async function AlertsPage({
  params,
  searchParams,
}: {
  params: Promise<{ organization: string; project: string }>;
  searchParams?: Promise<{ incident?: string | string[] }>;
}) {
  const { organization, project: slug } = await params;
  const { incident: asked } = (await searchParams) ?? {};
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  // The organization's roster names its admins, who act on every project
  // with no seat on it; only its owner and admins may read it.
  const role = activeOrganization(gate)?.role;
  const [access, listing, roster, everyone] = await Promise.all([
    fetchAlerts(project.id),
    fetchConnectors(project.id),
    fetchMembers(project.id),
    role === "owner" || role === "admin" ? fetchOrganizationMembers() : null,
  ]);
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's alerts"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }

  const followed =
    typeof asked === "string" && z.guid().safeParse(asked).success ? asked.toLowerCase() : null;
  let incidents: Incident[] = access.incidents;
  // A full list leaves out the oldest resolved: the one a trace links is
  // read on its own, and stands last, where the list's order puts it.
  if (
    followed &&
    incidents.length >= INCIDENTS_MAX &&
    !incidents.some((incident) => incident.id === followed)
  ) {
    const one = await fetchIncident(project.id, followed);
    if (one.kind === "ok") incidents = [...incidents, one.incident];
  }

  const connections: ConnectionChoice[] | null = listing
    ? connectionChoices(listing.connections)
    : null;
  const connectionNames = new Map((connections ?? []).map((c) => [c.id, c.name]));
  // A connection the listing lacks is one the project no longer has, unless
  // the listing could not be read at all.
  const unknownConnector = connections ? "a removed connector" : "a connector";
  const rosters = [
    everyone?.kind === "ok" ? everyone.members : [],
    roster.kind === "ok" ? roster.members : [],
  ];
  const people = new Map(
    rosters.flat().map((member) => [member.member_id, member.display_name || member.email]),
  );
  // Someone neither roster names has left, where the organization's was read;
  // else they may be an admin of it, seated on no project.
  const who = (id: string) =>
    people.get(id) ??
    (everyone?.kind === "ok"
      ? "a former member"
      : roster.kind === "ok"
        ? "an organization admin or a former member"
        : "someone");
  const now = access.at;
  const everyAlert = windowsFor(access.mutes, null, now);

  return (
    <>
      <PageHeader
        title="Alerts"
        action={
          <div className="flex items-center gap-3">
            <MuteButton projectId={project.id} ruleId={null} name={null} />
            <NewAlert projectId={project.id} connections={connections} />
          </div>
        }
      />
      <div className="grid gap-6">
        <PrecisionRow precision={access.precision} />

        <Section
          title="Incidents"
          description="Open ones first, then the newest. Any member of the project acknowledges one, which stops its reminders, or resolves it."
        >
          {incidents.length === 0 ? (
            <Empty
              title="No incidents."
              detail="An alert that crosses its threshold opens one here, and tells the project's feed and the connectors it links."
            />
          ) : (
            <DataTable>
              <THead>
                <Th>Alert</Th>
                <Th>Severity</Th>
                <Th>Status</Th>
                <Th>Opened</Th>
                <Th>Told</Th>
                <Th />
              </THead>
              <tbody>
                {incidents.map((incident) => {
                  const run = incident.runs.at(-1);
                  return (
                    <tr
                      key={incident.id}
                      id={`incident-${incident.id}`}
                      className={incident.id === followed ? "scroll-mt-3.5 bg-muted/60" : undefined}
                      aria-current={incident.id === followed ? "true" : undefined}
                      tabIndex={incident.id === followed ? -1 : undefined}
                    >
                      <Td className="min-w-48">
                        <div className="grid gap-0.5">
                          <a
                            href={`#alert-${incident.rule_id}`}
                            className="font-medium underline-offset-4 hover:underline"
                          >
                            {incident.rule_name}
                          </a>
                          {incident.reopened > 0 && (
                            <span className="text-xs text-muted-foreground">
                              Reopened {incident.reopened === 1 ? "once" : `${incident.reopened} times`}
                            </span>
                          )}
                          {run && (
                            <Link
                              href={traceHref(organization, slug, run)}
                              className="inline-flex w-fit items-center gap-0.5 text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
                              title="The run its last notice showed"
                            >
                              run {shortId(run)}
                              <ArrowUpRight className="size-3" aria-hidden="true" />
                            </Link>
                          )}
                        </div>
                      </Td>
                      <Td className="whitespace-nowrap">
                        <Badge variant={severityVariant(incident.severity)} className="font-normal">
                          {SEVERITY_LABEL[incident.severity]}
                        </Badge>
                      </Td>
                      <Td className="text-xs text-muted-foreground">
                        <IncidentState incident={incident} who={who} />
                      </Td>
                      <Td className="text-xs whitespace-nowrap text-muted-foreground">
                        <LocalTime iso={incident.opened_at} />
                      </Td>
                      <Td className="min-w-40 text-xs text-muted-foreground">
                        {toldWords(incident, connectionNames, unknownConnector).join(", ")}
                      </Td>
                      <Td className="text-right">
                        <IncidentActions
                          projectId={project.id}
                          incidentId={incident.id}
                          status={incident.status}
                          name={incident.rule_name}
                          openedAt={incident.opened_at}
                          voteHref={projectPath(organization, slug, `/alerts/incidents/${incident.id}/vote`)}
                        />
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </DataTable>
          )}
        </Section>

        <Section
          title="Alerts"
          headingId={ALERTS_HEADING}
          description="Each reads a measure of the project's spans over a window against its thresholds, every minute. Any member makes, edits, pauses, mutes and deletes them."
        >
          {everyAlert.length > 0 && (
            <Rows>
              {everyAlert.map((mute) => (
                <Row
                  key={mute.id}
                  label={inForce(mute, now) ? "Every alert is muted" : "Every alert will be muted"}
                  hint={<WindowWords mute={mute} who={who} now={now} />}
                >
                  <EndMute
                    projectId={project.id}
                    muteId={mute.id}
                    label="End the window muting every alert"
                    landing={ALERTS_HEADING}
                  />
                </Row>
              ))}
            </Rows>
          )}
          {access.alerts.length === 0 ? (
            <Empty
              title="No alerts yet."
              detail="Make one with New alert: the share of runs that fail, the p95 duration of model calls, the tokens they use, or what a run spends, over the last hour, day or week."
            />
          ) : (
            access.alerts.map((alert) => (
              <AlertCard
                key={alert.id}
                alert={alert}
                projectId={project.id}
                connections={connections}
                connectionNames={connectionNames}
                unknownConnector={unknownConnector.replace(/^a/, "A")}
                windows={windowsFor(access.mutes, alert.id, now)}
                precision={access.precision.rules.find((rule) => rule.rule_id === alert.id) ?? null}
                who={who}
                now={now}
              />
            ))
          )}
        </Section>
      </div>
    </>
  );
}

/** The week's votes on the project's incidents: the share calling them
 *  useful, which the alerts are tuned by. */
function PrecisionRow({ precision }: { precision: Precision }) {
  const words = precisionWords(precision);
  return (
    <Rows>
      <Row
        label="Precision"
        hint={`The share of the votes on the last ${precision.days} days' incidents that call them useful. Each notice links a page to vote from.`}
        keep
      >
        {words ? (
          <span className="grid gap-0.5 text-right">
            <span className="font-medium">{words.share}</span>
            <span className="text-xs text-muted-foreground">{words.votes}</span>
          </span>
        ) : (
          <span className="text-muted-foreground">No votes yet</span>
        )}
      </Row>
    </Rows>
  );
}

/** Where an incident stands, and who moved it there. */
function IncidentState({ incident, who }: { incident: Incident; who: (id: string) => string }) {
  if (incident.status === IncidentStatus.Resolved && incident.resolved_at) {
    return (
      <span className="grid gap-0.5">
        <span className="text-foreground">
          {incident.resolved_by ? `Resolved by ${who(incident.resolved_by)}` : "Recovered"}
        </span>
        <LocalTime iso={incident.resolved_at} />
      </span>
    );
  }
  if (incident.status === IncidentStatus.Acknowledged && incident.acknowledged_by) {
    return (
      <span className="grid gap-0.5">
        <span className="text-foreground">Acknowledged by {who(incident.acknowledged_by)}</span>
        {incident.acknowledged_at && <LocalTime iso={incident.acknowledged_at} />}
      </span>
    );
  }
  return <span className="text-foreground">{STATUS_LABEL[incident.status]}</span>;
}

/** A window's span and who set it. */
function WindowWords({ mute, who, now }: { mute: Mute; who: (id: string) => string; now: number }) {
  return (
    <>
      {inForce(mute, now) ? "Until " : "From "}
      {inForce(mute, now) ? (
        <LocalTime iso={mute.ends_at} />
      ) : (
        <>
          <LocalTime iso={mute.starts_at} /> until <LocalTime iso={mute.ends_at} />
        </>
      )}
      , set by {who(mute.created_by)}.
    </>
  );
}

function Empty({ title, detail }: { title: string; detail: string }) {
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <div className="grid gap-1 p-4">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{detail}</p>
      </div>
    </Card>
  );
}

/** One fact of an alert, named beside it. */
function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 wrap-break-word">{children}</dd>
    </>
  );
}

/** One alert: its name, where it stands and its controls, its metric in
 *  words, then what it reads, when it alerts, and where it is told. */
function AlertCard({
  alert,
  projectId,
  connections,
  connectionNames,
  unknownConnector,
  windows,
  precision,
  who,
  now,
}: {
  alert: Alert;
  projectId: string;
  connections: ConnectionChoice[] | null;
  connectionNames: ReadonlyMap<string, string>;
  /** What a linked connection the names lack is called. */
  unknownConnector: string;
  windows: Mute[];
  precision: Precision["rules"][number] | null;
  who: (id: string) => string;
  now: number;
}) {
  const thresholds = thresholdWords(alert);
  const statuses = new Map((connections ?? []).map((connection) => [connection.id, connection.status]));
  const filters = filterWords(alert.filters);
  const votes = precision ? precisionWords(precision) : null;
  return (
    <Card id={`alert-${alert.id}`} className="scroll-mt-4 gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid min-w-0 gap-1">
          <div className="flex min-w-0 items-center gap-2">
            <h3 id={alertHeading(alert.id)} tabIndex={-1} className="min-w-0 truncate text-sm font-medium">
              {alert.name}
            </h3>
            <Badge variant={severityVariant(alert.severity)} className="shrink-0 font-normal">
              {SEVERITY_LABEL[alert.severity]}
            </Badge>
          </div>
          <p className="text-sm text-muted-foreground">{metricWords(alert)}</p>
        </div>
        <AlertActions projectId={projectId} alert={alert} connections={connections} />
      </div>
      {/* On their first baselines, so a name stands level with a line whose
          button makes it taller. */}
      <dl className="grid items-baseline gap-x-3 gap-y-2 border-t border-border pt-3 text-sm sm:grid-cols-[9rem_minmax(0,1fr)]">
        <Detail label="Now">
          <Standing alert={alert} />
        </Detail>
        <Detail label="Thresholds">
          Alert {thresholds.alert}
          {thresholds.warning && <>, warning {thresholds.warning}</>}
        </Detail>
        <Detail label="Filters">{filters.length > 0 ? filters.join("; ") : "None"}</Detail>
        <Detail label="No data">{MISSING_DATA_LABEL[alert.missing_data]}</Detail>
        <Detail label="Notified again">{renotifyWords(alert.renotify_minutes)}</Detail>
        <Detail label="Connectors">
          {alert.connectors.length === 0 ? (
            "None: its incidents reach the project's feed alone"
          ) : (
            <ul className="grid gap-1.5">
              {alert.connectors.map((link) => {
                const name = connectionNames.get(link.connection_id) ?? unknownConnector;
                const status = statuses.get(link.connection_id);
                const stopped = status ? CONNECTION_STATUS_LABEL[status] : null;
                return (
                  <li key={link.connection_id} className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                    {/* A webhook's host is one word, which a phone's width must break. */}
                    <span className="min-w-0">
                      {name}
                      <span className="text-muted-foreground">
                        {" · "}
                        {LEVEL_LABEL[link.level].toLowerCase()}
                        {link.hours ? `, ${hoursWords(link.hours)}` : ""}
                      </span>
                    </span>
                    {/* A connection that is not active hears nothing, whatever the alert links. */}
                    {stopped && (
                      <Badge variant="secondary" className="font-normal">
                        {stopped}
                      </Badge>
                    )}
                    {link.muted && (
                      <>
                        <Badge variant="secondary" className="font-normal">
                          Muted after five failed deliveries
                        </Badge>
                        <HearAgain
                          projectId={projectId}
                          ruleId={alert.id}
                          connectionId={link.connection_id}
                          name={name}
                        />
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Detail>
        <Detail label="Maintenance">
          {windows.length === 0 ? (
            "None"
          ) : (
            <ul className="grid gap-1.5">
              {windows.map((mute) => (
                <li key={mute.id} className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                  <span>
                    {mute.rule_id === null ? "Every alert: " : ""}
                    <WindowWords mute={mute} who={who} now={now} />
                  </span>
                  <EndMute
                    projectId={projectId}
                    muteId={mute.id}
                    label={
                      mute.rule_id === null
                        ? "End the window muting every alert"
                        : `End the window muting ${alert.name}`
                    }
                    landing={alertHeading(alert.id)}
                  />
                </li>
              ))}
            </ul>
          )}
        </Detail>
        <Detail label="Precision">
          {votes ? `${votes.share}, ${votes.votes}` : "No votes yet"}
        </Detail>
      </dl>
    </Card>
  );
}

/** Where an alert stands: paused, and by whom; not yet evaluated; or its
 *  last value, and since when it has stood where it does. */
function Standing({ alert }: { alert: Alert }) {
  if (alert.paused === "plan") {
    return (
      <>
        Paused by the plan: the organization has more alerts than its plan holds, and it runs
        again once there is room.
      </>
    );
  }
  if (alert.paused === "person") {
    return (
      <>
        Paused{alert.paused_at && <> since <LocalTime iso={alert.paused_at} /></>}.
      </>
    );
  }
  if (alert.severity === Severity.Unknown) {
    return <>Not evaluated yet: it is, within a minute.</>;
  }
  // Saved over since its last reading, which answered another question.
  if (alert.evaluated_at === null) {
    return (
      <>
        Changed since its last reading, taken again within a minute;{" "}
        {SEVERITY_LABEL[alert.severity]} until then.
      </>
    );
  }
  return (
    <>
      {alert.value === null
        ? "No span in its window"
        : amount(alert.measure, alert.aggregation, alert.value)}
      , read <LocalTime iso={alert.evaluated_at} />; {SEVERITY_LABEL[alert.severity]} since{" "}
      <LocalTime iso={alert.severity_since} />.
    </>
  );
}
