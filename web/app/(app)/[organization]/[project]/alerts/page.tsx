import type { ReactNode } from "react";
import { ArrowUpRight } from "lucide-react";
import Link from "next/link";
import { z } from "zod";

import { AccessDenied } from "@/components/access-denied";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { Row, Rows } from "@/components/rows";
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
  incidentKindsLabel,
  inForce,
  metricWords,
  renotifyWords,
  severityVariant,
  thresholdWords,
  toldWords,
  windowsFor,
  type ConnectionChoice,
} from "@/lib/alerts";
import { memberNames, ownerContact } from "@/lib/identity";
import { sole } from "@/lib/list-query";
import { administers } from "@/lib/roles";
import {
  INCIDENTS_MAX,
  activeOrganization,
  fetchAlerts,
  fetchConnectors,
  fetchIncident,
  fetchMembers,
  fetchOrganizationMembers,
  type Alert,
  type Incident,
  type Mute,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { shortId, traceHref } from "@/lib/traces";
import { IncidentStatus, Severity } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

import { AlertActions, HearAgain } from "./_alert-actions";
import { NewAlert } from "./_alert-panel";
import { IncidentActions } from "./_incident-actions";
import { EndMute, MuteButton } from "./_mute";

export const metadata = { title: "Alerts" };

// The project's alerts, built as Langfuse's are: every alert with its
// metric, filters, window, thresholds, severity, connectors and maintenance
// windows, each made, edited, paused, muted and deleted here, and under each
// the incidents it opened, acknowledged or resolved by any member. A trace a
// notice showed links back with `?incident=`, which draws that incident
// under its alert even past the list.
export default async function AlertsPage({
  params,
  searchParams,
}: PageProps<"/[organization]/[project]/alerts">) {
  const { organization, project: slug } = await params;
  const { incident } = await searchParams;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  // The organization's roster names its admins, who act on every project
  // with no seat on it; only its owner and admins may read it.
  const [access, listing, roster, everyone] = await Promise.all([
    fetchAlerts(project.id),
    fetchConnectors(project.id),
    fetchMembers(project.id),
    administers(activeOrganization(gate)?.role) ? fetchOrganizationMembers() : null,
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

  const connections: ConnectionChoice[] | null = listing
    ? connectionChoices(listing.connections)
    : null;
  const connectionNames = new Map((connections ?? []).map((c) => [c.id, c.name]));
  // A connection the listing lacks is one the project no longer has, unless
  // the listing could not be read at all.
  const unknownConnector = connections ? "a removed connector" : "a connector";
  const who = memberNames(
    everyone?.kind === "ok" ? everyone.members : null,
    roster.kind === "ok" ? roster.members : null,
  );
  const now = access.at;
  const everyAlert = windowsFor(access.mutes, null, now);

  const asked = sole(incident);
  const followed = asked && z.guid().safeParse(asked).success ? asked.toLowerCase() : null;
  let incidents: Incident[] = access.incidents;
  // A full list leaves out the oldest resolved: the one a trace links is
  // read on its own, and stands last under its alert, where the list's order
  // puts it.
  if (
    followed &&
    incidents.length >= INCIDENTS_MAX &&
    !incidents.some((incident) => incident.id === followed)
  ) {
    const one = await fetchIncident(project.id, followed);
    if (one.kind === "ok") incidents = [...incidents, one.incident];
  }
  const incidentsOf = Map.groupBy(incidents, (incident) => incident.rule_id);

  return (
    <>
      <PageHeader
        title="Alerts"
        headingId={ALERTS_HEADING}
        info="Each reads a measure of the project's spans over a window against its thresholds, every minute, with the incidents it opened under it, open ones first. Any member makes, edits, pauses, mutes and deletes alerts, and acknowledges or resolves their incidents."
        action={
          <div className="flex items-center gap-3">
            {/* A window over every alert mutes nothing until there is one. */}
            {access.alerts.length > 0 && (
              <MuteButton projectId={project.id} ruleId={null} name={null} />
            )}
            <NewAlert projectId={project.id} connections={connections} />
          </div>
        }
      />
      <div className="grid gap-6">
        <section className="grid gap-3">
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
            <NoticeCard
              title="No alerts yet."
              body="Make one with New alert: the share of runs that fail, the p95 duration of model calls, the tokens they use, or what a run spends, over the last hour, day or week."
            />
          ) : (
            access.alerts.map((alert) => (
              <AlertCard
                key={alert.id}
                alert={alert}
                organization={organization}
                slug={slug}
                projectId={project.id}
                connections={connections}
                connectionNames={connectionNames}
                unknownConnector={unknownConnector}
                windows={windowsFor(access.mutes, alert.id, now)}
                incidents={incidentsOf.get(alert.id) ?? []}
                followed={followed}
                who={who}
                now={now}
              />
            ))
          )}
        </section>
      </div>
    </>
  );
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
 *  words, then what it reads, when it alerts, where it is told, and the
 *  incidents it opened. */
function AlertCard({
  alert,
  organization,
  slug,
  projectId,
  connections,
  connectionNames,
  unknownConnector,
  windows,
  incidents,
  followed,
  who,
  now,
}: {
  alert: Alert;
  organization: string;
  slug: string;
  projectId: string;
  connections: ConnectionChoice[] | null;
  connectionNames: ReadonlyMap<string, string>;
  /** What a linked connection the names lack is called, mid-sentence. */
  unknownConnector: string;
  windows: Mute[];
  /** Its incidents, those not resolved first, then the newest. */
  incidents: Incident[];
  /** The incident a trace's link asked for, if any. */
  followed: string | null;
  who: (id: string) => string;
  now: number;
}) {
  const thresholds = thresholdWords(alert);
  const choices = new Map((connections ?? []).map((connection) => [connection.id, connection]));
  const filters = filterWords(alert.filters);
  const read = readSince(alert);
  return (
    <Card id={`alert-${alert.id}`} className="scroll-mt-4 gap-3">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid min-w-0 gap-1">
          <div className="flex min-w-0 items-center gap-2">
            <h2 id={alertHeading(alert.id)} tabIndex={-1} className="min-w-0 truncate text-sm font-medium">
              {alert.name}
            </h2>
            <Badge variant={severityVariant(alert.severity)} className="shrink-0 font-normal">
              {/* Read, an alert keeping its last severity can still have none to keep. */}
              {alert.severity === Severity.Unknown && read ? "No severity" : SEVERITY_LABEL[alert.severity]}
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
          <Standing alert={alert} read={read} />
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
                const name =
                  connectionNames.get(link.connection_id) ?? unknownConnector.replace(/^a/, "A");
                const connection = choices.get(link.connection_id);
                const stopped = connection ? CONNECTION_STATUS_LABEL[connection.status] : null;
                const taken = connection ? incidentKindsLabel(connection.eventKinds) : null;
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
                    {/* A webhook hears only the kinds of notice its events name. */}
                    {taken && (
                      <Badge variant="secondary" className="font-normal">
                        {taken}
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
        <Detail label="Incidents">
          {incidents.length === 0 ? (
            "None"
          ) : (
            <ul className="grid gap-3">
              {incidents.map((incident) => (
                <IncidentItem
                  key={incident.id}
                  incident={incident}
                  followed={incident.id === followed}
                  organization={organization}
                  slug={slug}
                  projectId={projectId}
                  told={toldWords(incident, connectionNames, unknownConnector).join(", ")}
                  who={who}
                />
              ))}
            </ul>
          )}
        </Detail>
      </dl>
    </Card>
  );
}

/** One incident of an alert: its severity, where it stands and its controls,
 *  then when it opened, where its notices went and the run its last showed. */
function IncidentItem({
  incident,
  followed,
  organization,
  slug,
  projectId,
  told,
  who,
}: {
  incident: Incident;
  /** Whether a trace's link asked for it, which marks it. */
  followed: boolean;
  organization: string;
  slug: string;
  projectId: string;
  /** Where its notices went, in words. */
  told: string;
  who: (id: string) => string;
}) {
  const run = incident.runs.at(-1);
  return (
    <li
      id={`incident-${incident.id}`}
      className={cn(
        "grid scroll-mt-4 gap-1 rounded-sm",
        // A bar past 3:1 on either theme's page, and a border: the focus the link hands it clears a ring.
        followed && "-my-1 -mr-1 -ml-2 border-l-2 border-muted-foreground bg-muted/60 py-1 pr-1 pl-1.5",
      )}
      aria-current={followed ? "true" : undefined}
      tabIndex={followed ? -1 : undefined}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <Badge variant={severityVariant(incident.severity)} className="font-normal">
          {SEVERITY_LABEL[incident.severity]}
        </Badge>
        <IncidentState incident={incident} who={who} />
        <div className="ml-auto">
          <IncidentActions
            projectId={projectId}
            ruleId={incident.rule_id}
            incidentId={incident.id}
            status={incident.status}
            name={incident.rule_name}
            openedAt={incident.opened_at}
          />
        </div>
      </div>
      <span className="text-xs text-muted-foreground">
        Opened <LocalTime iso={incident.opened_at} />
        {incident.reopened > 0 &&
          `, reopened ${incident.reopened === 1 ? "once" : `${incident.reopened} times`}`}
        {` · told ${told}`}
        {run && (
          <>
            {" · "}
            <Link
              href={traceHref(organization, slug, run)}
              className="inline-flex items-center gap-0.5 underline-offset-4 hover:text-foreground hover:underline"
              title="The run its last notice showed"
            >
              run {shortId(run)}
              <ArrowUpRight className="size-3" aria-hidden="true" />
            </Link>
          </>
        )}
      </span>
    </li>
  );
}

/** Where an incident stands, who moved it there, and when. */
function IncidentState({ incident, who }: { incident: Incident; who: (id: string) => string }) {
  if (incident.status === IncidentStatus.Resolved && incident.resolved_at) {
    return (
      <span>
        {incident.resolved_by ? `Resolved by ${who(incident.resolved_by)}` : "Recovered"}
        <span className="text-muted-foreground">
          {" · "}
          <LocalTime iso={incident.resolved_at} />
        </span>
      </span>
    );
  }
  if (incident.status === IncidentStatus.Acknowledged && incident.acknowledged_by) {
    return (
      <span>
        Acknowledged by {who(incident.acknowledged_by)}
        {incident.acknowledged_at && (
          <span className="text-muted-foreground">
            {" · "}
            <LocalTime iso={incident.acknowledged_at} />
          </span>
        )}
      </span>
    );
  }
  return <span>{STATUS_LABEL[incident.status]}</span>;
}

/** When the alert was last read, if after its severity was last set: one the
 *  plan runs again keeps the reading from before its pause. */
function readSince(alert: Alert): string | null {
  const at = alert.evaluated_at;
  return at !== null && Date.parse(at) >= Date.parse(alert.severity_since) ? at : null;
}

/** Where an alert stands: paused, and by whom; not yet read; read with no
 *  severity to keep; or its last value, and since when it has stood there. */
function Standing({ alert, read }: { alert: Alert; read: string | null }) {
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
  if (read === null) {
    if (alert.severity === Severity.Unknown) return <>Not evaluated yet: it is, within a minute.</>;
    // Saved over since its last reading, which answered another question.
    return (
      <>
        Changed since its last reading, taken again within a minute;{" "}
        {SEVERITY_LABEL[alert.severity]} until then.
      </>
    );
  }
  // Only an alert that keeps its last severity is read and still has none.
  if (alert.severity === Severity.Unknown) {
    return (
      <>
        Its window has held no span yet, so it has no severity to keep; read{" "}
        <LocalTime iso={read} />.
      </>
    );
  }
  return (
    <>
      {alert.value === null
        ? "No span in its window"
        : amount(alert.measure, alert.aggregation, alert.value)}
      , read <LocalTime iso={read} />; {SEVERITY_LABEL[alert.severity]} since{" "}
      <LocalTime iso={alert.severity_since} />.
    </>
  );
}
