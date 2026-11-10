"use client";

import { Pagination, type PagePatch } from "@/components/pagination";

/** An audit page's URL at a page and a row count: `base` with each named,
 *  one left out being the first page or the default rows. */
export function auditHref(base: string, { page, limit }: PagePatch): string {
  const query = new URLSearchParams();
  if (page !== undefined) query.set("page", String(page));
  if (limit !== undefined) query.set("limit", String(limit));
  const text = query.toString();
  return text ? `${base}?${text}` : base;
}

// The audit log's foot, on the organization's page and a project's: `base` is
// the page's own path, since a function cannot cross from the server's render.
export function AuditPagination({
  base,
  total,
  page,
  limit,
}: {
  base: string;
  total: number;
  page: number;
  limit: number;
}) {
  return (
    <Pagination total={total} page={page} limit={limit} href={(patch) => auditHref(base, patch)} />
  );
}
