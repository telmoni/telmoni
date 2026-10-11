import Link from "next/link";
import { ChevronDown, ChevronUp } from "lucide-react";

import { Th } from "@/components/data-table";
import { SortDirection } from "@/lib/types/enums";

// A heading that sorts: the list's URL with this column first, flipped on a
// second click, and `aria-sort` for the reader who cannot see the chevron.
// The traces, users and conversations lists share it, each with its own columns.
export function SortHeading<S extends string>({
  label,
  column,
  sort,
  dir,
  href,
}: {
  label: string;
  column: S;
  sort: S;
  dir: SortDirection;
  href: string;
}) {
  const active = sort === column;
  const ariaSort = active ? (dir === SortDirection.Asc ? "ascending" : "descending") : "none";
  return (
    <Th className="whitespace-nowrap" aria-sort={ariaSort}>
      <Link href={href} className="inline-flex items-center gap-1 hover:underline">
        {label}
        {active &&
          (dir === SortDirection.Asc ? (
            <ChevronUp className="size-3.5" aria-hidden="true" />
          ) : (
            <ChevronDown className="size-3.5" aria-hidden="true" />
          ))}
      </Link>
    </Th>
  );
}
