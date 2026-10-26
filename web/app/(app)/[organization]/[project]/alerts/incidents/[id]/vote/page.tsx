import Link from "next/link";

import { AccessDenied } from "@/components/access-denied";
import { LocalTime } from "@/components/local-time";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { SEVERITY_LABEL, STATUS_LABEL } from "@/lib/alerts";
import { ownerContact } from "@/lib/identity";
import {
  activeOrganization,
  fetchIncident,
  fetchProjectBySlug,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";

import { VoteButtons } from "./_vote";

export const metadata = { title: "Incident" };

// The page every notice of an incident links to: the incident as it stands,
// then the two words a member picks between. Opening it casts nothing, since
// a mail scanner or a chat's preview follows the link too; the vote is the
// POST its buttons send.
export default async function IncidentVotePage({
  params,
}: {
  params: Promise<{ organization: string; project: string; id: string }>;
}) {
  const { organization, project: slug, id } = await params;
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;

  const alerts = projectPath(organization, slug, "/alerts");
  const access = await fetchIncident(project.id, id);
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
  if (access.kind === "missing") {
    return (
      <>
        <PageHeader title="Incident" backHref={alerts} backLabel="Alerts" />
        <Card className="grid gap-1 text-sm">
          <p className="font-medium">No incident with this id in this project.</p>
          <p className="text-muted-foreground">
            Its alert may have been deleted, or the incident belongs to another project.{" "}
            <Link href={alerts} className="underline underline-offset-4">
              Back to the alerts
            </Link>
            .
          </p>
        </Card>
      </>
    );
  }

  const { incident } = access;
  return (
    <>
      <PageHeader title={incident.rule_name} backHref={alerts} backLabel="Alerts" />
      <Card className="grid gap-3 text-sm">
        <p>
          <span className="font-medium">{SEVERITY_LABEL[incident.severity]}</span>,{" "}
          {STATUS_LABEL[incident.status].toLowerCase()}; opened <LocalTime iso={incident.opened_at} />
          {incident.resolved_at && (
            <>
              , resolved <LocalTime iso={incident.resolved_at} />
            </>
          )}
          .
        </p>
        <p className="font-medium">Was this worth being told about?</p>
        <VoteButtons projectId={project.id} incidentId={incident.id} yours={incident.yours} />
        <p className="text-muted-foreground">
          {incident.useful} useful, {incident.noise} noise so far. Each member has one vote,
          changed by picking the other word.
        </p>
      </Card>
    </>
  );
}
