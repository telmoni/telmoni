import { InfoIcon } from "lucide-react";
import type { MouseEvent, ReactNode } from "react";

import { Th } from "@/components/data-table";
import { SortHeading } from "@/components/sort-heading";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { DownloadFormat } from "@/lib/server/entities/traces";
import type { Density } from "@/lib/trace-prefs";
import type { ColumnSpec } from "@/lib/traces";
import type { SortDirection } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// What the three list tables, their pickers and their downloads share, so a
// heading, a row's click or a hint is drawn and read one way.

/** The headings of the columns a table shows: a sortable one a link that
 *  sorts by it, the rest plain, counts standing right. */
export function ColumnHeadings<K extends string, S extends string>({
  columns,
  sort,
  dir,
  sortHref,
}: {
  columns: readonly ColumnSpec<K, S>[];
  sort: S;
  dir: SortDirection;
  sortHref: (column: S) => string;
}) {
  return columns.map((column) =>
    column.sort ? (
      <SortHeading
        key={column.key}
        label={column.label}
        column={column.sort}
        sort={sort}
        dir={dir}
        href={sortHref(column.sort)}
        className={column.numeric ? "text-right" : undefined}
      />
    ) : (
      <Th key={column.key} className={cn("whitespace-nowrap", column.numeric && "text-right")}>
        {column.label}
      </Th>
    ),
  );
}

/** The cells' padding at the density chosen. */
export function compactPadding(density: Density): string | undefined {
  return density === "compact" ? "py-1.5" : undefined;
}

/** A row's look: a pointer, and marked while it is the one open. */
export function rowClass(current: boolean): string {
  return cn("cursor-pointer", current ? "bg-muted" : "hover:bg-muted/50");
}

/** Whether a click on a row was a control's or a link's own, which keep
 *  their own way, or a drag that chose some text to copy. */
export function rowClickTaken(event: MouseEvent<HTMLElement>): boolean {
  if ((event.target as HTMLElement).closest("a, button, input, label")) return true;
  return Boolean(window.getSelection()?.toString());
}

/** Whether a click on a link is a plain one: not one asking for a new tab or
 *  window, or a copy of the link, which are the link's own to answer. */
export function plainClick(event: MouseEvent<HTMLElement>): boolean {
  return (
    event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey
  );
}

/** How many columns show, the id's counted among them. */
export function columnsCounted(shown: number, total: number): string {
  return `${shown + 1}/${total + 1}`;
}

/** `columns` with `key` taken off where it shows, else added. */
export function toggledColumn<K extends string>(columns: readonly K[], key: K): K[] {
  return columns.includes(key) ? columns.filter((c) => c !== key) : [...columns, key];
}

/** What a column means: on hover beside its name, and as the copy `id`
 *  names, which describes its item to a screen reader. */
export function ColumnHint({
  id,
  text,
  className,
}: {
  id: string;
  text: string;
  className?: string;
}): ReactNode {
  return (
    <>
      <span id={id} hidden>
        {text}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className={cn("pointer-events-auto text-muted-foreground", className)} aria-hidden="true">
            <InfoIcon className="size-3.5" />
          </span>
        </TooltipTrigger>
        <TooltipContent side="right" className="max-w-56">
          {text}
        </TooltipContent>
      </Tooltip>
    </>
  );
}

/** The two shapes a list comes down in, each a menu item. */
export function DownloadItems({ href }: { href: (format: DownloadFormat) => string }) {
  return (
    <>
      <DropdownMenuItem asChild>
        <a href={href("csv")} download>
          CSV, for a spreadsheet
        </a>
      </DropdownMenuItem>
      <DropdownMenuItem asChild>
        <a href={href("json")} download>
          JSON
        </a>
      </DropdownMenuItem>
    </>
  );
}
