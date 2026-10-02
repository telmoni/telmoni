import { AccessDenied } from "@/components/access-denied";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import {
  activeOrganization,
  fetchAuditEvents,
  fetchProjectBySlug,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { Role } from "@/lib/types/enums";

import { AuditLog } from "../../../_audit-log";

export const metadata = { title: "Audit log" };

export default async function AuditPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;
  const [gate, project] = await Promise.all([
    getServerContext(),
    fetchProjectBySlug(slug),
  ]);
  if (!gate) return <ServiceUnavailable />;
  const overview = projectPath(organization, slug);

  const canReadAudit =
    project?.role === Role.Owner || project?.role === Role.Admin;
  if (!project || !canReadAudit) {
    return (
      <AccessDenied
        what="this project's audit log"
        owner={ownerContact(activeOrganization(gate))}
        backHref={overview}
      />
    );
  }

  const access = await fetchAuditEvents(project.id);

  return (
    <AuditLog
      access={access}
      what="this project's audit log"
      owner={ownerContact(activeOrganization(gate))}
      backHref={overview}
      description={
        "Who did what, when — every mutation on this project, written append-only " +
        "in the same transaction as the change it records. Each row carries the " +
        "hash of the one before it, so an export can be re-verified by somebody " +
        "who does not trust us. The most recent 100 events are shown."
      }
      empty={
        "Actions on this project — minting a key, inviting a member, changing a " +
        "role — will appear here."
      }
    />
  );
}
