"use client";

import { useState } from "react";
import Link from "next/link";
import { ChevronDownIcon, SearchIcon, XIcon } from "lucide-react";

import { LinkPending } from "@/components/link-pending";
import { useListNavigation } from "@/components/traces/navigate";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { MONITOR_STATES, STATE_LABEL, monitorsHref, type MonitorQuery } from "@/lib/monitors";
import { formatCount } from "@/lib/traces";
import type { MonitorState } from "@/lib/types/enums";

// What narrows the Monitors list, all of it in the URL: the states picked,
// each with how many of the project's monitors stand in it, as the trace
// list's facets count their values; and words the names hold, sent as the
// person presses Enter or leaves the box.
export function MonitorFilters({
  organization,
  slug,
  query,
  counts,
}: {
  organization: string;
  slug: string;
  query: MonitorQuery;
  counts: Partial<Record<MonitorState, number>>;
}) {
  const { go, pending } = useListNavigation();
  const [typed, setTyped] = useState(query.q ?? "");
  const picked = query.state ?? [];
  const search = () => {
    const q = typed.trim() || undefined;
    if (q !== query.q) go(monitorsHref(organization, slug, query, { q }));
  };
  const pick = (state: MonitorState, on: boolean) => {
    const next = MONITOR_STATES.filter((s) => (s === state ? on : picked.includes(s)));
    go(monitorsHref(organization, slug, query, { state: next.length > 0 ? next : undefined }));
  };
  const narrowed = picked.length > 0 || query.q !== undefined;

  return (
    <div
      className="flex flex-wrap items-center gap-2"
      aria-busy={pending}
      data-pending={pending ? "" : undefined}
    >
      <form
        role="search"
        className="relative w-full sm:w-64"
        onSubmit={(e) => {
          e.preventDefault();
          search();
        }}
      >
        <SearchIcon
          className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
          aria-hidden="true"
        />
        <Input
          type="search"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onBlur={search}
          placeholder="Search by name"
          aria-label="Search monitors by name"
          maxLength={60}
          className="pl-8"
        />
      </form>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline">
            State
            {picked.length > 0 && (
              <Badge variant="secondary" className="px-1.5 tabular-nums">
                {picked.length}
              </Badge>
            )}
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-56">
          {MONITOR_STATES.map((state) => (
            <DropdownMenuCheckboxItem
              key={state}
              checked={picked.includes(state)}
              onCheckedChange={(on) => pick(state, on === true)}
              onSelect={(e) => e.preventDefault()}
            >
              <span>{STATE_LABEL[state]}</span>
              <span className="ml-auto text-xs text-muted-foreground tabular-nums">
                {formatCount(counts[state] ?? 0)}
              </span>
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {picked.map((state) => (
        <Badge key={state} variant="secondary" className="gap-1 font-normal">
          {STATE_LABEL[state]}
          <Link
            href={monitorsHref(organization, slug, query, {
              state: picked.length > 1 ? picked.filter((s) => s !== state) : undefined,
            })}
            scroll={false}
            aria-label={`Clear the state ${STATE_LABEL[state].toLowerCase()}`}
            className="rounded-sm text-muted-foreground hover:text-foreground"
          >
            <XIcon className="size-3" aria-hidden="true" />
            <LinkPending />
          </Link>
        </Badge>
      ))}
      {narrowed && (
        <Button asChild variant="ghost" size="sm">
          <Link
            href={monitorsHref(organization, slug, {}, {})}
            scroll={false}
            onClick={() => setTyped("")}
          >
            Clear all
            <LinkPending />
          </Link>
        </Button>
      )}
    </div>
  );
}
