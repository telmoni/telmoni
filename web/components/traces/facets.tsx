"use client";

import { useState } from "react";
import { ChevronRightIcon, PanelLeftIcon, SearchIcon } from "lucide-react";
import Link from "next/link";

import { LinkPending } from "@/components/link-pending";
import { useListNavigation } from "@/components/traces/navigate";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Facet, TraceFacets, TraceQuery } from "@/lib/server/entities/traces";
import {
  FILTERS,
  FILTER_LABEL,
  STATUS_LABEL,
  TYPED_FILTERS,
  activeFilters,
  fits,
  formatCount,
  isTagPair,
  toggled,
  tracesHref,
  unfiltered,
  without,
  type FilterKey,
  type TraceUi,
} from "@/lib/traces";
import type { SpanStatus } from "@/lib/types/enums";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

/** How many of a facet's values it lists before a search box above them
 *  earns its place. */
const SEARCHABLE_FROM = 8;

/** How many values the server names of a facet, the most runs first. */
const FACET_VALUES_MAX = 50;

/** The value a facet's list starts on: no item's, cmdk trimming each, so none is chosen and
 *  scrolled into view as it mounts, which in Chrome moves where the page's first Tab starts. */
const NONE_CHOSEN = " ";

type Props = {
  organization: string;
  slug: string;
  query: TraceQuery;
  ui: TraceUi;
  /** Each filter's values and their runs, or `null` where they could not
   *  be read. */
  facets: TraceFacets | null;
};

// The filters, beside the list as Langfuse's sidebar stands beside its
// tables: each a section of the values the window's runs carry, with how
// many runs carry each under every other filter, ticked to narrow the list
// to them. Ids — users and sessions — can be typed in as well, since the
// fifty a facet names are rarely the one a person is after.
export function TraceFacetsPanel({ organization, slug, query, ui, facets }: Props) {
  const { filters: shown } = useTracePrefs();
  const [search, setSearch] = useState("");
  const { go, pending } = useListNavigation();
  // Hidden, the panel leaves its button where its heading stood, as
  // Langfuse's sidebar folds to its icon, so it opens again from there.
  if (!shown) {
    return (
      <div className="shrink-0">
        <PanelButton shown={false} />
      </div>
    );
  }

  const ticked = activeFilters(query).length;
  const needle = search.trim().toLowerCase();
  const sections = FILTERS.filter((filter) => {
    if (!needle) return true;
    if (FILTER_LABEL[filter].toLowerCase().includes(needle)) return true;
    return facets?.[filter].values.some((v) => v.value.toLowerCase().includes(needle)) ?? false;
  });

  // As wide as the console's rail rows ("the sidebar apps", `--spacing-sidebar-inner`),
  // and that width whether the rail is open or closed.
  return (
    <aside
      className="grid w-full shrink-0 content-start gap-2 md:w-sidebar-inner"
      aria-label="Filters"
      aria-busy={pending}
      data-pending={pending ? "" : undefined}
    >
      <div className="flex h-9 items-center gap-2">
        <PanelButton shown />
        <h2 className="text-sm font-medium">Filters</h2>
        {ticked > 0 && (
          <Badge variant="secondary" className="px-1.5 tabular-nums">
            {ticked}
          </Badge>
        )}
        {ticked > 0 && (
          <Button asChild variant="ghost" size="xs" className="ml-auto">
            <Link href={tracesHref(organization, slug, unfiltered(query), {}, ui)} scroll={false}>
              Clear all
              <LinkPending />
            </Link>
          </Button>
        )}
      </div>
      <div className="relative">
        <SearchIcon
          className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
          aria-hidden="true"
        />
        <Input
          type="search"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search filters"
          aria-label="Search filters"
          className="pl-8"
        />
      </div>
      {facets === null ? (
        <p className="text-sm text-muted-foreground">The filters&rsquo; values could not be read.</p>
      ) : (
        <div className="grid">
          {sections.map((filter) => (
            <FacetSection
              key={filter}
              filter={filter}
              facet={facets[filter]}
              ticked={query[filter] ?? []}
              clearHref={tracesHref(organization, slug, without(query, filter), {}, ui)}
              onToggle={(value) => go(tracesHref(organization, slug, toggled(query, filter, value), {}, ui))}
            />
          ))}
        </div>
      )}
    </aside>
  );
}

/** The panel's own button, beside its heading: hides it, or opens it again. */
function PanelButton({ shown }: { shown: boolean }) {
  const label = shown ? "Hide the filters" : "Show the filters";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="text-muted-foreground hover:text-foreground"
          aria-label={label}
          aria-expanded={shown}
          onClick={() => setTracePrefs({ filters: !shown })}
        >
          <PanelLeftIcon />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** One filter's section: its values to tick, or — for an id — one typed in. */
function FacetSection({
  filter,
  facet,
  ticked,
  clearHref,
  onToggle,
}: {
  filter: FilterKey;
  facet: Facet;
  ticked: readonly string[];
  clearHref: string;
  onToggle: (value: string) => void;
}) {
  const label = FILTER_LABEL[filter];
  const typed = TYPED_FILTERS.includes(filter);
  // Open only where there is activity: a value ticked, or values among the
  // window's runs to pick from, but never an id's fifty alone. Open and
  // empty, a section only says so, and pushes the ones that hold something
  // down the panel. Read from the data until the reader opens or shuts it
  // themselves, since the page stays mounted as runs land and the window
  // moves, and a section shut while it was empty would stay shut.
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? (ticked.length > 0 || (!typed && facet.values.length > 0));
  const [mode, setMode] = useState<"select" | "text">("select");
  // A ticked value the window's runs no longer carry stays on the list, at
  // nought, so it can be unticked where it was ticked.
  const values = [
    ...facet.values,
    ...ticked
      .filter((value) => !facet.values.some((v) => v.value === value))
      .map((value) => ({ value, runs: 0 })),
  ];
  const text = (value: string) => (filter === "status" ? STATUS_LABEL[value as SpanStatus] : value);

  return (
    <Collapsible open={open} onOpenChange={setChosen} className="border-b border-border py-1">
      <div className="flex items-center gap-1">
        <CollapsibleTrigger asChild>
          <Button variant="ghost" size="sm" className="flex-1 justify-start px-1.5">
            <ChevronRightIcon
              className={cn("text-muted-foreground transition-transform", open && "rotate-90")}
              aria-hidden="true"
            />
            {label}
            {ticked.length > 0 && (
              <Badge variant="secondary" className="px-1.5 tabular-nums">
                {ticked.length}
              </Badge>
            )}
          </Button>
        </CollapsibleTrigger>
        {ticked.length > 0 && (
          <Button asChild variant="ghost" size="xs">
            <Link href={clearHref} scroll={false} aria-label={`Clear the ${label.toLowerCase()} filter`}>
              Clear
              <LinkPending />
            </Link>
          </Button>
        )}
      </div>
      <CollapsibleContent className="grid gap-2 pb-2">
        {typed && (
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={mode}
            onValueChange={(next) => next && setMode(next as "select" | "text")}
            className="w-full"
            aria-label={`Pick or type a ${label.toLowerCase()}`}
          >
            <ToggleGroupItem value="select" className="flex-1">
              Select
            </ToggleGroupItem>
            <ToggleGroupItem value="text" className="flex-1">
              Text
            </ToggleGroupItem>
          </ToggleGroup>
        )}
        {mode === "text" ? (
          <TypedValue filter={filter} label={label} onAdd={onToggle} />
        ) : values.length === 0 ? (
          <p className="px-1.5 text-xs text-muted-foreground">
            No {label.toLowerCase()} among the window&rsquo;s runs.
          </p>
        ) : (
          <Command className="bg-transparent" defaultValue={NONE_CHOSEN}>
            {values.length > SEARCHABLE_FROM && (
              <CommandInput placeholder={`Search ${label.toLowerCase()}s`} />
            )}
            <CommandList className="max-h-64">
              <CommandEmpty className="py-3 text-center text-xs text-muted-foreground">
                No {label.toLowerCase()} matches.
              </CommandEmpty>
              {values.map((v) => {
                const on = ticked.includes(v.value);
                return (
                  <CommandItem key={v.value} value={v.value} onSelect={() => onToggle(v.value)}>
                    <Checkbox checked={on} tabIndex={-1} aria-hidden="true" className="pointer-events-none" />
                    <span className="min-w-0 flex-1 truncate" title={v.value}>
                      {text(v.value)}
                    </span>
                    {on && <span className="sr-only">(selected)</span>}
                    <span className="text-xs tabular-nums text-muted-foreground">{formatCount(v.runs)}</span>
                  </CommandItem>
                );
              })}
            </CommandList>
          </Command>
        )}
        {facet.more && mode === "select" && (
          <p className="px-1.5 text-xs text-muted-foreground">
            The {FACET_VALUES_MAX} with the most runs{typed ? "; type one in to find another" : ""}.
          </p>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** An id typed in — or a tag, as `key:value` — added to the filter as the
 *  list is narrowed to it. */
function TypedValue({
  filter,
  label,
  onAdd,
}: {
  filter: FilterKey;
  label: string;
  onAdd: (value: string) => void;
}) {
  const [value, setValue] = useState("");
  const trimmed = value.trim();
  const tag = filter === "tag";
  const asked = tag ? "A tag, as key:value" : `A ${label.toLowerCase()}'s id`;
  const taken =
    trimmed !== "" && (filter === "status" || fits(filter, trimmed)) && (!tag || isTagPair(trimmed));
  return (
    <form
      className="flex gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (!taken) return;
        onAdd(trimmed);
        setValue("");
      }}
    >
      <Input
        value={value}
        onChange={(event) => setValue(event.target.value)}
        placeholder={asked}
        aria-label={asked}
        autoComplete="off"
        spellCheck={false}
        className="font-mono text-xs"
      />
      <Button type="submit" variant="outline" disabled={!taken}>
        Add
      </Button>
    </form>
  );
}
