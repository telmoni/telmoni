import { AccessDenied } from "@/components/access-denied";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import {
  activeOrganization,
  fetchAuditEvents,
  fetchProject,
  getServerContext,
} from "@/lib/server/data";
import { Role } from "@/lib/types/enums";

import { AuditLog } from "../../_audit-log";

export const metadata = { title: "Audit log" };

export default async function AuditPage({
  params,
}: {
  params: Promise<{ projectId?: string }>;
}) {
  const { projectId = "" } = await params;
  const [gate, project] = await Promise.all([
    getServerContext(),
    fetchProject(projectId),
  ]);
  if (!gate) return <ServiceUnavailable />;

  const canReadAudit =
    project?.role === Role.Owner || project?.role === Role.Admin;
  if (!project || !canReadAudit) {
    return (
      <AccessDenied
        what="this project's audit log"
        owner={ownerContact(activeOrganization(gate))}
        backHref={`/${projectId}`}
      />
    );
  }

  const access = await fetchAuditEvents(projectId);

  return (
    <AuditLog
      access={access}
      what="this project's audit log"
      owner={ownerContact(activeOrganization(gate))}
      backHref={`/${projectId}`}
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
