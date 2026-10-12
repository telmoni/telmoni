"use client";

import {
  ChevronLeftIcon,
  ChevronRightIcon,
  ChevronsLeftIcon,
  ChevronsRightIcon,
  type LucideIcon,
} from "lucide-react";
import Link from "next/link";
import { useOptimistic } from "react";

import { LinkPending } from "@/components/link-pending";
import { useListNavigation } from "@/components/traces/navigate";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { TraceQuery } from "@/lib/server/entities/traces";
import {
  DEFAULT_ROWS,
  ROWS_PER_PAGE,
  formatCount,
  pageCount,
  tracesHref,
  type TraceUi,
} from "@/lib/traces";

type Props = {
  organization: string;
  slug: string;
  query: TraceQuery;
  ui: TraceUi;
  total: number;
  page: number;
  limit: number;
};

// The foot of the list, as Langfuse's is: how many traces it holds, how many
// a page shows, which page this is, and the way to the others — the first
// and the last a click away too.
export function TracePagination({ organization, slug, query, ui, total, page, limit }: Props) {
  const { go, pending } = useListNavigation();
  // The rows picked show at once, and give way to the server's when it lands.
  const [rowsShown, showRows] = useOptimistic(limit);
  const pages = pageCount(total, limit);
  const to = (p: number) =>
    tracesHref(organization, slug, query, { page: p > 1 ? p : undefined }, ui);
  return (
    <nav
      className="flex flex-wrap items-center justify-end gap-x-6 gap-y-2 text-sm"
      aria-label="Pages"
      aria-busy={pending}
      data-pending={pending ? "" : undefined}
    >
      <span className="mr-auto text-muted-foreground">
        Total {formatCount(total)}
      </span>
      <div className="flex items-center gap-2">
        <span className="font-medium">Rows per page</span>
        <Select
          value={String(rowsShown)}
          onValueChange={(value) => {
            const rows = Number(value);
            go(
              tracesHref(
                organization,
                slug,
                query,
                { limit: rows === DEFAULT_ROWS ? undefined : rows, page: undefined },
                ui,
              ),
              () => showRows(rows),
            );
          }}
        >
          <SelectTrigger size="sm" aria-label="Rows per page">
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="end">
            {ROWS_PER_PAGE.map((rows) => (
              <SelectItem key={rows} value={String(rows)}>
                {rows}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <span className="font-medium tabular-nums">
        Page {formatCount(page)} of {formatCount(pages)}
      </span>
      <div className="flex items-center gap-1">
        <PageButton href={to(1)} disabled={page <= 1} label="First page" icon={ChevronsLeftIcon} />
        <PageButton href={to(page - 1)} disabled={page <= 1} label="Previous page" icon={ChevronLeftIcon} />
        <PageButton href={to(page + 1)} disabled={page >= pages} label="Next page" icon={ChevronRightIcon} />
        <PageButton href={to(pages)} disabled={page >= pages} label="Last page" icon={ChevronsRightIcon} />
      </div>
    </nav>
  );
}

function PageButton({
  href,
  disabled,
  label,
  icon: Icon,
}: {
  href: string;
  disabled: boolean;
  label: string;
  icon: LucideIcon;
}) {
  if (disabled) {
    return (
      <Button variant="outline" size="icon" disabled aria-label={label}>
        <Icon />
      </Button>
    );
  }
  return (
    <Button asChild variant="outline" size="icon">
      <Link href={href} scroll={false} aria-label={label}>
        <Icon />
        <LinkPending />
      </Link>
    </Button>
  );
}
