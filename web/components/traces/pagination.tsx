"use client";

import { Pagination } from "@/components/pagination";
import type { ListQuery } from "@/lib/server/entities/traces";
import { listPageHref, type ListName, type TraceUi } from "@/lib/traces";

type Props = {
  list: ListName;
  organization: string;
  slug: string;
  query: ListQuery<string>;
  /** The trace list's own state, which its links carry. */
  ui?: TraceUi;
  total: number;
  page: number;
  limit: number;
};

// A trace list's foot: the shared one, its links the list's own URL with its
// filters and state kept.
export function ListPagination({ list, organization, slug, query, ui = {}, total, page, limit }: Props) {
  return (
    <Pagination
      total={total}
      page={page}
      limit={limit}
      href={(patch) =>
        listPageHref(list, organization, slug, { ...query, page: patch.page, limit: patch.limit }, ui)
      }
    />
  );
}
