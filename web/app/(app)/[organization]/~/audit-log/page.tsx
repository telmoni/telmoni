import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { ownerContact } from "@/lib/identity";
import { administersOrganization } from "@/lib/organization-role";
import {
  activeOrganization,
  fetchOrganizationAudit,
  getServerContext,
  identityContext,
} from "@/lib/server/data";
import { organizationPath } from "@/lib/slug";

import { AuditLog } from "../../../_audit-log";

export const metadata = { title: "Audit log" };

export default async function OrganizationAuditPage() {
  const [gate, ident] = await Promise.all([
    getServerContext(),
    identityContext(),
  ]);
  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);
  const overview = organization ? organizationPath(organization.slug) : "/console";

  if (!organization || !ident || !administersOrganization(ident.role)) {
    return (
      <AccessDenied
        what="this organization's audit log"
        owner={ownerContact(organization)}
        backHref={overview}
      />
    );
  }

  return (
    <AuditLog
      access={await fetchOrganizationAudit()}
      what="this organization's audit log"
      owner={ownerContact(organization)}
      backHref={overview}
      description={
        "Who did what, when — everything in this organization: every project's " +
        "actions and the ones outside any project, such as a session revoked or a " +
        "member seated. Written append-only in the " +
        "same transaction as the change it records; each row carries the hash of " +
        "the one before it, so an export can be re-verified by somebody who does " +
        "not trust us. The most recent 100 events are shown."
      }
      empty={
        "Actions across the organization — a project's keys and members, its own " +
        "roster and its sessions — will appear here."
      }
    />
  );
}
