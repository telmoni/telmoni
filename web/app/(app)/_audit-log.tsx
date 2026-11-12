import { History } from "lucide-react";
import type { ReactNode } from "react";

import { AccessDenied } from "@/components/access-denied";
import { AuditPagination } from "@/components/audit-pagination";
import { AuditTable } from "@/components/audit-table";
import { PageHeader } from "@/components/page-header";
import { NoticeCard } from "@/components/notice-card";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { whole } from "@/lib/list-query";
import { isOneOf } from "@/lib/one-of";
import type { AuditAccess, AuditPaging } from "@/lib/server/entities/audit";
import { DEFAULT_ROWS, MAX_PAGE, ROWS_PER_PAGE } from "@/lib/traces";

/** The page and the rows an audit page's URL names: one of the footer's row
 *  counts, else the default; the first page where it names none. */
export function auditPaging(query: {
  page?: string | string[];
  limit?: string | string[];
}): AuditPaging {
  const rows = whole(query.limit, MAX_ROWS);
  const page = whole(query.page, MAX_PAGE);
  return {
    limit: isOneOf(ROWS_PER_PAGE, rows) ? rows : DEFAULT_ROWS,
    page: page !== undefined && page > 1 ? page : undefined,
  };
}

const MAX_ROWS = Math.max(...ROWS_PER_PAGE);

export function AuditLog({
  access,
  base,
  what,
  owner = null,
  description,
  empty,
  backHref,
  backLabel,
  action,
}: {
  access: AuditAccess;
  /// The page's own path, which its pages' links are spelled from.
  base: string;
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

  const { events, total, page, limit } = access;

  return (
    <>
      <PageHeader title="Audit log" action={action} />
      <div className="grid gap-page">
        <Section title="Activity" description={description}>
          <div className="grid gap-gutter">
            {events.length > 0 ? (
              <AuditTable events={events} />
            ) : total === 0 ? (
              <NoticeCard icon={History} title="No events yet." body={empty} />
            ) : (
              <NoticeCard title="No events on this page." body="The log ends before it." />
            )}
            {total > 0 && <AuditPagination base={base} total={total} page={page} limit={limit} />}
          </div>
        </Section>
      </div>
    </>
  );
}
