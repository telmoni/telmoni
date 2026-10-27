import Link from "next/link";
import { XIcon } from "lucide-react";

import { LinkPending } from "@/components/link-pending";
import { Badge } from "@/components/ui/badge";
import type { ListQuery } from "@/lib/server/entities/traces";
import {
  activeFilters,
  listPageHref,
  unfiltered,
  without,
  type ListName,
  type TraceUi,
} from "@/lib/traces";

// The filters in force above a list: a value at a time, each taken off by
// its own ×, and all of them at once. Plain links, so the bar is the
// server's to draw.
export function FilterBar({
  list,
  organization,
  slug,
  query,
  ui = {},
}: {
  list: ListName;
  organization: string;
  slug: string;
  query: ListQuery<string>;
  /** The trace list's own state, which its links carry. */
  ui?: TraceUi;
}) {
  const chips = activeFilters(query);
  if (chips.length === 0) return null;
  return (
    <div
      className="flex min-h-9 flex-wrap items-center gap-1.5 rounded-md border border-border px-2 py-1"
      aria-label="Filters in force"
    >
      {chips.map((chip) => (
        <Badge key={`${chip.filter}:${chip.value}`} variant="secondary" className="max-w-full gap-1 font-normal">
          <span className="text-muted-foreground">{chip.label}:</span>
          <span className="max-w-56 truncate" title={chip.value}>
            {chip.text}
          </span>
          <Link
            href={listPageHref(list, organization, slug, without(query, chip.filter, chip.value), ui)}
            scroll={false}
            aria-label={`Clear ${chip.label.toLowerCase()} ${chip.text}`}
            className="rounded-sm text-muted-foreground hover:text-foreground"
          >
            <XIcon className="size-3" aria-hidden="true" />
            <LinkPending />
          </Link>
        </Badge>
      ))}
      <Link
        href={listPageHref(list, organization, slug, unfiltered(query), ui)}
        scroll={false}
        className="ml-auto text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
      >
        Clear all
        <LinkPending />
      </Link>
    </div>
  );
}
