import type { ReactNode } from "react";

import { AccessDenied } from "@/components/access-denied";
import { AuditTable } from "@/components/audit-table";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
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
      <div className="grid gap-6">
        <section className="grid gap-3">
          <div className="grid gap-1">
            <h2 className="text-sm font-medium">Activity</h2>
            <p className="text-sm text-muted-foreground">{description}</p>
          </div>

          {events.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">No events yet.</p>
                <p className="text-sm text-muted-foreground">{empty}</p>
              </div>
            </Card>
          ) : (
            <AuditTable events={events} />
          )}
        </section>
      </div>
    </>
  );
}
