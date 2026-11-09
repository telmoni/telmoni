import type { ReactNode } from "react";

import { AccessDenied } from "@/components/access-denied";
import { AuditTable } from "@/components/audit-table";
import { PageHeader } from "@/components/page-header";
import { NoticeCard } from "@/components/notice-card";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import type { AuditAccess } from "@/lib/server/entities/audit";

export function AuditLog({
  access,
  what,
  owner = null,
  description,
  empty,
  backHref,
  backLabel,
  action,
}: {
  access: AuditAccess;
  what: string;
  /// Whom the refusal tells the caller to ask (`ownerContact`).
  owner?: string | null;
  description: string;
  empty: string;
  backHref: string;
  backLabel?: string;
  /// The page header's control, where every page keeps its action: Export,
  /// on the organization's log.
  action?: ReactNode;
}) {
  if (access.kind === "unavailable") return <ServiceUnavailable />;
  if (access.kind === "forbidden") {
    return (
      <AccessDenied
        what={what}
        owner={owner}
        backHref={backHref}
        backLabel={backLabel}
      />
    );
  }

  const events = access.events;

  return (
    <>
      <PageHeader title="Audit log" action={action} />
      <div className="grid gap-page">
        <Section title="Activity" description={description}>
          {events.length === 0 ? (
            <NoticeCard title="No events yet." body={empty} />
          ) : (
            <AuditTable events={events} />
          )}
        </Section>
      </div>
    </>
  );
}
