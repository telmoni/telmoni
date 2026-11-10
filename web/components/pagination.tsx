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
import { DEFAULT_ROWS, ROWS_PER_PAGE, formatCount, pageCount } from "@/lib/traces";

/** What a link of the footer changes: a value left out is the first page, or
 *  the default rows. */
export type PagePatch = { page?: number; limit?: number };

// The foot of a list: how many rows it holds, how many a page shows, which
// page this is, and the way to the others — the first and the last a click
// away too. `href` spells the list's URL for a page and a row count.
export function Pagination({
  total,
  page,
  limit,
  href,
}: {
  total: number;
  page: number;
  limit: number;
  href: (patch: PagePatch) => string;
}) {
  const { go, pending } = useListNavigation();
  // The rows picked show at once, and give way to the server's when it lands.
  const [rowsShown, showRows] = useOptimistic(limit);
  const pages = pageCount(total, limit);
  const rows = limit === DEFAULT_ROWS ? undefined : limit;
  const to = (p: number) => href({ page: p > 1 ? p : undefined, limit: rows });
  return (
    <nav
      className="flex flex-wrap items-center justify-end gap-x-page gap-y-2 text-sm"
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
            const picked = Number(value);
            go(href({ limit: picked === DEFAULT_ROWS ? undefined : picked }), () =>
              showRows(picked),
            );
          }}
        >
          <SelectTrigger aria-label="Rows per page">
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="end">
            {ROWS_PER_PAGE.map((option) => (
              <SelectItem key={option} value={String(option)}>
                {option}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <span className="font-medium tabular-nums">
        Page {formatCount(page)} of {formatCount(pages)}
      </span>
      <div className="flex items-center gap-gutter">
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
