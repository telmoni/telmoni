"use client";

import { SearchIcon } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState } from "react";

import { Input } from "@/components/ui/input";
import type { ListQuery } from "@/lib/server/entities/traces";
import { SEARCH_MAX, listPageHref, type ListName, type TraceUi } from "@/lib/traces";

/** A search of the runs' kept content, beside the filters in force: whole
 *  words, a phrase in order, within the list's window and filters. */
export function ContentSearch({
  list,
  organization,
  slug,
  query,
  ui,
}: {
  list: ListName;
  organization: string;
  slug: string;
  query: ListQuery<string>;
  ui: TraceUi;
}) {
  const router = useRouter();
  const [value, setValue] = useState(query.q ?? "");
  return (
    <form
      role="search"
      className="relative min-w-48 flex-1 sm:max-w-72"
      onSubmit={(e) => {
        e.preventDefault();
        const q = value.trim();
        router.push(
          listPageHref(list, organization, slug, { ...query, q: q || undefined, page: undefined }, ui),
          { scroll: false },
        );
      }}
    >
      <SearchIcon
        className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground"
        aria-hidden="true"
      />
      <Input
        type="search"
        aria-label="Search kept content"
        placeholder="Search kept content"
        maxLength={SEARCH_MAX}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        className="h-8 pl-8"
      />
    </form>
  );
}
