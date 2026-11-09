import Link from "next/link";
import { CheckIcon, ChevronDown, ChevronUp } from "lucide-react";

import { Th } from "@/components/data-table";
import { LinkPending } from "@/components/link-pending";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SortDirection } from "@/lib/types/enums";
import { tagSortKey } from "@/lib/types/traces";
import { cn } from "@/lib/utils";

// A heading that sorts: the list's URL with this column first, flipped on a
// second click, and `aria-sort` for the reader who cannot see the chevron.
// The list stays where it was scrolled to, as it does for every other control
// of a list, and dims while the sorted one is on its way.
export function SortHeading<S extends string>({
  label,
  column,
  sort,
  dir,
  href,
  className,
}: {
  label: string;
  column: S;
  sort: S;
  dir: SortDirection;
  href: string;
  /** A number's column stands right, its heading with it. */
  className?: string;
}) {
  const active = sort === column;
  return (
    <Th className={cn("whitespace-nowrap", className)} aria-sort={ariaSort(active, dir)}>
      <Link href={href} scroll={false} className="inline-flex items-center gap-1 hover:underline">
        {label}
        {active && <SortChevron dir={dir} />}
        <LinkPending />
      </Link>
    </Th>
  );
}

/** The heading of a column that sorts by one tag's value: a menu of `keys`,
 *  each a link to the list sorted by it, flipped on a second pick as a
 *  heading is; the key in force named beside it, and offered though the
 *  window's tags no longer hold it. */
export function TagSortHeading({
  label,
  sort,
  dir,
  keys,
  href,
}: {
  label: string;
  /** The sort in force. */
  sort: string;
  dir: SortDirection;
  keys: readonly string[];
  href: (key: string) => string;
}) {
  const sorted = tagSortKey(sort);
  const offered = sorted === undefined || keys.includes(sorted) ? keys : [...keys, sorted].sort();
  return (
    <Th className="whitespace-nowrap" aria-sort={ariaSort(sorted !== undefined, dir)}>
      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label={sorted === undefined ? label : `${label}, sorted by ${sorted}`}
          className="inline-flex cursor-pointer items-center gap-1 hover:underline"
        >
          {label}
          {sorted !== undefined && (
            <>
              <span className="max-w-32 truncate font-normal text-muted-foreground">{sorted}</span>
              <SortChevron dir={dir} />
            </>
          )}
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-64">
          <DropdownMenuLabel>Sort by tag</DropdownMenuLabel>
          {offered.length === 0 ? (
            <DropdownMenuItem disabled>No tags in this window</DropdownMenuItem>
          ) : (
            offered.map((key) => (
              <DropdownMenuItem key={key} asChild>
                <Link href={href(key)} scroll={false} aria-current={key === sorted ? "true" : undefined}>
                  <CheckIcon className={cn(key !== sorted && "invisible")} aria-hidden="true" />
                  <span className="truncate">{key}</span>
                </Link>
              </DropdownMenuItem>
            ))
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </Th>
  );
}

function ariaSort(active: boolean, dir: SortDirection): "none" | "ascending" | "descending" {
  if (!active) return "none";
  return dir === SortDirection.Asc ? "ascending" : "descending";
}

function SortChevron({ dir }: { dir: SortDirection }) {
  return dir === SortDirection.Asc ? (
    <ChevronUp className="size-icon" aria-hidden="true" />
  ) : (
    <ChevronDown className="size-icon" aria-hidden="true" />
  );
}
