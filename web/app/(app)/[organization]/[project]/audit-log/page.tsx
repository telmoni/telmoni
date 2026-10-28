import { AccessDenied } from "@/components/access-denied";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import { activeOrganization, fetchAuditEvents } from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { administers } from "@/lib/roles";

import { AuditLog } from "../../../_audit-log";

export const metadata = { title: "Audit log" };

export default async function AuditPage({
  params,
}: PageProps<"/[organization]/[project]/audit-log">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;
  const overview = projectPath(organization, slug);

  if (!administers(project.role)) {
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
