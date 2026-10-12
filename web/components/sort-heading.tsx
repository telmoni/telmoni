import Link from "next/link";
import { ChevronDown, ChevronUp } from "lucide-react";

import { Th } from "@/components/data-table";
import { LinkPending } from "@/components/link-pending";
import { SortDirection } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// A heading that sorts: the list's URL with this column first, flipped on a
// second click, and `aria-sort` for the reader who cannot see the chevron.
// The traces, users and sessions lists share it, each with its own columns.
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
  const ariaSort = active ? (dir === SortDirection.Asc ? "ascending" : "descending") : "none";
  return (
    <Th className={cn("whitespace-nowrap", className)} aria-sort={ariaSort}>
      <Link href={href} scroll={false} className="inline-flex items-center gap-1 hover:underline">
        {label}
        {active &&
          (dir === SortDirection.Asc ? (
            <ChevronUp className="size-3.5" aria-hidden="true" />
          ) : (
            <ChevronDown className="size-3.5" aria-hidden="true" />
          ))}
        <LinkPending />
      </Link>
    </Th>
  );
}
