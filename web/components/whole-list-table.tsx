"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useState, type ReactNode } from "react";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { NoticeCard } from "@/components/notice-card";
import { SortHeading } from "@/components/sort-heading";
import { compactPadding } from "@/components/traces/list-table";
import { Input } from "@/components/ui/input";
import { sortAfterClick, sortInForce, type Column } from "@/lib/list-query";
import { SortDirection } from "@/lib/types/enums";
import { useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

// A list the page holds whole — a project's evaluators, its rules — drawn as
// the three span lists draw theirs: the same table, headings that sort by a
// link, the density the person chose. Sorted and searched here, since such a
// list is tens of rows, never a page of thousands.

/** One column: its heading, its cell, and the value it sorts by, where it sorts. */
export interface WholeColumn<T> {
  key: string;
  label: string;
  cell: (row: T) => ReactNode;
  sort?: { by: (row: T) => string | number; first: SortDirection };
  numeric?: boolean;
  className?: string;
}

export function WholeListTable<T>({
  rows,
  columns,
  rowKey,
  defaultSort,
  searchOf,
  searchLabel,
  actions,
}: {
  rows: readonly T[];
  columns: readonly WholeColumn<T>[];
  rowKey: (row: T) => string;
  /** The column the list stands in until a heading is clicked. */
  defaultSort: string;
  /** What a row is found by. */
  searchOf: (row: T) => string;
  searchLabel: string;
  /** A row's own buttons, at its end. */
  actions?: (row: T) => ReactNode;
}) {
  const pathname = usePathname();
  const params = useSearchParams();
  const tight = compactPadding(useTracePrefs().density);
  const [search, setSearch] = useState("");

  const sorts: Column<string>[] = columns.flatMap((c) =>
    c.sort ? [{ sort: c.key, label: c.label, first: c.sort.first }] : [],
  );
  const asked = params.get("sort");
  const dirAsked = params.get("dir");
  const query = {
    sort: sorts.some((s) => s.sort === asked) ? (asked ?? undefined) : undefined,
    dir: dirAsked === SortDirection.Asc || dirAsked === SortDirection.Desc ? dirAsked : undefined,
  };
  const { sort, dir } = sortInForce(sorts, defaultSort, query);
  const hrefOf = (column: string) => {
    const next = new URLSearchParams(params);
    const { sort: by, dir: way } = sortAfterClick(sorts, defaultSort, query, column);
    next.set("sort", by);
    next.set("dir", way);
    return `${pathname}?${next}`;
  };

  const by = columns.find((c) => c.key === sort)?.sort?.by;
  const typed = search.trim().toLowerCase();
  const shown = rows
    .filter((row) => !typed || searchOf(row).toLowerCase().includes(typed))
    .toSorted((a, b) => {
      if (!by) return 0;
      const [x, y] = [by(a), by(b)];
      const order = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y));
      return dir === SortDirection.Asc ? order : -order;
    });

  return (
    <div className="grid gap-gutter">
      <Input
        type="search"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        placeholder="Search"
        aria-label={searchLabel}
        className="max-w-md"
      />
      {shown.length === 0 ? (
        <NoticeCard title="Nothing matches." body="Nothing here holds what was searched for." />
      ) : (
        <DataTable>
          <THead>
            {columns.map((column) =>
              column.sort ? (
                <SortHeading
                  key={column.key}
                  label={column.label}
                  column={column.key}
                  sort={sort}
                  dir={dir}
                  href={hrefOf(column.key)}
                  className={column.numeric ? "text-right" : undefined}
                />
              ) : (
                <Th key={column.key} className={cn("whitespace-nowrap", column.numeric && "text-right")}>
                  {column.label}
                </Th>
              ),
            )}
            {actions && <Th />}
          </THead>
          <tbody>
            {shown.map((row) => (
              <tr key={rowKey(row)}>
                {columns.map((column) => (
                  <Td
                    key={column.key}
                    className={cn(tight, column.numeric && "text-right tabular-nums", column.className)}
                  >
                    {column.cell(row)}
                  </Td>
                ))}
                {actions && <Td className={cn("text-right whitespace-nowrap", tight)}>{actions(row)}</Td>}
              </tr>
            ))}
          </tbody>
        </DataTable>
      )}
    </div>
  );
}
