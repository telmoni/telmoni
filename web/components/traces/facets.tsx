"use client";

import { useRef, useState, type CSSProperties } from "react";
import { ChevronRightIcon, ListIndentDecrease, ListIndentIncrease, SearchIcon, XIcon } from "lucide-react";
import Link from "next/link";

import { LinkPending } from "@/components/link-pending";
import { useListNavigation } from "@/components/traces/navigate";
import { FiltersEdge, useFiltersWidth } from "@/components/traces/panel-width";
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
import { storedIdAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import type { Facet, Facets, ListQuery } from "@/lib/server/entities/traces";
import {
  FILTERS,
  FILTER_LABEL,
  FAULT_LABEL,
  PRIMARY_FILTERS,
  ROW_NOUN,
  SCORE_FILTER_HINT,
  SCORE_NAME_MAX,
  STATUS_LABEL,
  STORED_ID_FILTERS,
  TYPED_FILTERS,
  activeFilters,
  fits,
  formatCount,
  isTagPair,
  listPageHref,
  toggled,
  unfiltered,
  without,
  type FilterKey,
  type ListName,
  type StoredIdFilter,
} from "@/lib/traces";
import { isOneOf } from "@/lib/one-of";
import { SpanFault, SpanStatus } from "@/lib/types/enums";
import { useToastAction } from "@/lib/use-action";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";
import { decimalTyped } from "@/lib/typed-number";

const STATUSES = Object.values(SpanStatus);

/** How many of a facet's values it lists before a search box above them
 *  earns its place. */
const SEARCHABLE_FROM = 8;

/** How many values the server names of a facet, the most rows first. */
const FACET_VALUES_MAX = 50;

/** The value a facet's list starts on: no item's, cmdk trimming each, so none is chosen and
 *  scrolled into view as it mounts, which in Chrome moves where the page's first Tab starts. */
const NONE_CHOSEN = " ";

type Props = {
  /** The list the panel narrows. */
  list: ListName;
  organization: string;
  slug: string;
  /** The project an id typed in is looked up in. */
  projectId: string;
  query: ListQuery<string>;
  /** Each filter's values and how many of the list's rows carry each, or
   *  `null` where they could not be read. */
  facets: Facets | null;
};

// The filters beside a list: each the values the window's traces carry, with
// how many of the list's rows carry each under every other filter, ticked to
// narrow the list. It opens on the few most narrowing starts from, the rest
// behind More. Ids can be typed in as well, since the fifty a facet names are
// rarely the one a person is after.
export function FacetsPanel({ list, organization, slug, projectId, query, facets }: Props) {
  const { filters: shown, moreFilters } = useTracePrefs();
  const [search, setSearch] = useState("");
  const { go, pending } = useListNavigation();
  const panel = useRef<HTMLElement>(null);
  const { width, edge } = useFiltersWidth(panel);
  const href = (next: ListQuery<string>) => listPageHref(list, organization, slug, next);
  // Hidden, the panel leaves its button where its heading stood, so it opens
  // again from there.
  if (!shown) {
    return (
      <div className="shrink-0">
        <PanelButton shown={false} />
      </div>
    );
  }

  const ticked = activeFilters(query).length;
  const needle = search.trim().toLowerCase();
  // A search reads every filter; else those the panel opens with, every one
  // once More is opened, and any in force, so it can be cleared where it was set.
  const every = moreFilters || needle !== "";
  const sections = FILTERS.filter((filter) => {
    if (!needle) return every || PRIMARY_FILTERS.includes(filter) || (query[filter]?.length ?? 0) > 0;
    if (FILTER_LABEL[filter].toLowerCase().includes(needle)) return true;
    return facets?.[filter].values.some((v) => v.value.toLowerCase().includes(needle)) ?? false;
  });
  const scored = needle ? "score".includes(needle) : every || query.score !== undefined;

  // As wide as the main sidebar's links until dragged (`w-filters`, `globals.css`),
  // and never past half the row, so the list keeps its room.
  return (
    <aside
      ref={panel}
      style={{ "--filters-width": width === null ? undefined : `${width}px` } as CSSProperties}
      className="relative grid w-full shrink-0 content-start gap-gutter *:min-w-0 md:w-filters md:max-w-1/2"
      aria-label="Filters"
      aria-busy={pending}
      data-pending={pending ? "" : undefined}
    >
      <div className="flex h-control items-center gap-gutter">
        <PanelButton shown />
        <h2 className="min-w-0 truncate text-sm font-medium">Filters</h2>
        {ticked > 0 && (
          <Badge variant="secondary" className="tabular-nums">
            {ticked}
          </Badge>
        )}
        {ticked > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button asChild variant="ghost" size="icon" className="ml-auto">
                <Link
                  href={href(unfiltered(query))}
                  scroll={false}
                  aria-label="Clear all filters"
                >
                  <XIcon />
                  <LinkPending />
                </Link>
              </Button>
            </TooltipTrigger>
            <TooltipContent>Clear all filters</TooltipContent>
          </Tooltip>
        )}
      </div>
      <div className="relative">
        <SearchIcon
          className="pointer-events-none absolute top-1/2 left-gutter size-icon -translate-y-1/2 text-muted-foreground"
          aria-hidden="true"
        />
        <Input
          type="search"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search filters"
          aria-label="Search filters"
          // Its text a gutter past the icon, the border's pixel counted.
          className="pl-[calc(2*var(--spacing-gutter)+var(--spacing-icon)-1px)]"
        />
      </div>
      {/* Score in the same gap-free stack as the facets, so it sits one rule below the last. */}
      <div className="grid *:min-w-0">
        {facets === null ? (
          <p className="text-sm text-muted-foreground">The filters&rsquo; values could not be read.</p>
        ) : (
          sections.map((filter) => (
            <FacetSection
              key={filter}
              filter={filter}
              facet={facets[filter]}
              rows={ROW_NOUN[list][1]}
              projectId={projectId}
              ticked={query[filter] ?? []}
              clearHref={href(without(query, filter))}
              onToggle={(value) => go(href(toggled(query, filter, value)))}
            />
          ))
        )}
        {scored && (
          <ScoreSection
            // Drawn afresh as the URL's score changes: a clear, a step back, a view.
            key={`${query.score ?? ""}/${query.score_min ?? ""}/${query.score_max ?? ""}`}
            list={list}
            query={query}
            clearHref={href(without(query, "score"))}
            onScore={(score) => go(href({ ...query, ...score, page: undefined }))}
          />
        )}
        {!needle && (
          <Button
            variant="ghost"
            size="sm"
            className="justify-start text-muted-foreground"
            aria-expanded={moreFilters}
            onClick={() => setTracePrefs({ moreFilters: !moreFilters })}
          >
            <ChevronRightIcon className={cn("transition-transform", moreFilters && "rotate-90")} aria-hidden="true" />
            {moreFilters ? "Fewer filters" : "More filters"}
          </Button>
        )}
      </div>
      {/* Last in the panel, so Tab reaches it after the filters, between them and the list. */}
      <FiltersEdge edge={edge} />
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
          {/* The main rail's toggle: in while shown, out while hidden. */}
          {shown ? <ListIndentDecrease /> : <ListIndentIncrease />}
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
  rows,
  projectId,
  ticked,
  clearHref,
  onToggle,
}: {
  filter: FilterKey;
  facet: Facet;
  /** What the list's rows are, as the counts beside the values count them. */
  rows: string;
  projectId: string;
  ticked: readonly string[];
  clearHref: string;
  onToggle: (value: string) => void;
}) {
  const label = FILTER_LABEL[filter];
  const typed = TYPED_FILTERS.includes(filter);
  // An id is stored hashed, so one is typed as the application sent it.
  const stored = isOneOf(STORED_ID_FILTERS, filter) ? filter : null;
  // Open only where there is activity: a value ticked, or values among the
  // window's traces to pick from, but never an id's fifty alone. Open and
  // empty, a section only says so, and pushes the ones that hold something
  // down the panel. Read from the data until the reader opens or shuts it
  // themselves, since the page stays mounted as traces land and the window
  // moves, and a section shut while it was empty would stay shut.
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? (ticked.length > 0 || (!typed && facet.values.length > 0));
  const [mode, setMode] = useState<"select" | "text">("select");
  // A ticked value the window's traces no longer carry stays on the list, at
  // nought, so it can be unticked where it was ticked.
  const values = [
    ...facet.values,
    ...ticked
      .filter((value) => !facet.values.some((v) => v.value === value))
      .map((value) => ({ value, count: 0 })),
  ];
  const text = (value: string) =>
    filter === "status" && isOneOf(STATUSES, value) ? STATUS_LABEL[value] : filter === "fault" && isOneOf(Object.values(SpanFault), value) ? FAULT_LABEL[value] : value;

  return (
    <Collapsible open={open} onOpenChange={setChosen} className="border-b border-border">
      {/* Its Clear is an X: a word at the panel's edge stretched the row. */}
      <div className="flex items-center gap-gutter">
        <CollapsibleTrigger asChild>
          <Button variant="ghost" size="sm" className="min-w-0 flex-1 justify-start">
            <ChevronRightIcon
              className={cn("text-muted-foreground transition-transform", open && "rotate-90")}
              aria-hidden="true"
            />
            <span className="truncate">{label}</span>
            {ticked.length > 0 && (
              <Badge variant="secondary" className="tabular-nums">
                {ticked.length}
              </Badge>
            )}
          </Button>
        </CollapsibleTrigger>
        {ticked.length > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button asChild variant="ghost" size="icon">
                <Link
                  href={clearHref}
                  scroll={false}
                  aria-label={`Clear the ${label.toLowerCase()} filter`}
                >
                  <XIcon />
                  <LinkPending />
                </Link>
              </Button>
            </TooltipTrigger>
            <TooltipContent>Clear</TooltipContent>
          </Tooltip>
        )}
      </div>
      <CollapsibleContent className="grid gap-2 pb-2">
        {typed && (
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={mode}
            onValueChange={(next) => {
              if (next === "select" || next === "text") setMode(next);
            }}
            className="w-full"
            aria-label={`Pick or type a ${label.toLowerCase()}`}
          >
            <ToggleGroupItem value="select" className="flex-1">
              Select
            </ToggleGroupItem>
            <ToggleGroupItem value="text" className="flex-1">
              {stored ? "Your id" : "Text"}
            </ToggleGroupItem>
          </ToggleGroup>
        )}
        {mode === "text" && stored ? (
          <StoredId filter={stored} label={label} projectId={projectId} ticked={ticked} onAdd={onToggle} />
        ) : mode === "text" ? (
          <TypedTag onAdd={onToggle} />
        ) : values.length === 0 ? (
          <p className="px-gutter text-xs text-muted-foreground">
            No {label.toLowerCase()} among the window&rsquo;s traces.
          </p>
        ) : (
          <Command className="group/facet bg-transparent" defaultValue={NONE_CHOSEN}>
            {values.length > SEARCHABLE_FROM && (
              <CommandInput placeholder={`Search ${label.toLowerCase()}s`} />
            )}
            <CommandList className="max-h-64">
              <CommandEmpty className="py-gutter text-center text-xs text-muted-foreground">
                No {label.toLowerCase()} matches.
              </CommandEmpty>
              {values.map((v) => {
                const on = ticked.includes(v.value);
                // cmdk's cursor stays on the last row touched and a click leaves the
                // list focused, so it is painted only under the pointer or with
                // the keyboard (`:focus-visible`); the tick alone says what is on.
                return (
                  <CommandItem
                    key={v.value}
                    value={v.value}
                    onSelect={() => onToggle(v.value)}
                    // The box under the heading's chevron, the value under its label.
                    className="gap-gutter px-gutter data-[selected=true]:bg-transparent data-[selected=true]:text-foreground data-[selected=true]:hover:bg-accent/50 group-focus-visible/facet:data-[selected=true]:bg-accent/50 group-has-focus-visible/facet:data-[selected=true]:bg-accent/50"
                  >
                    <Checkbox checked={on} tabIndex={-1} aria-hidden="true" className="pointer-events-none" />
                    <span className="min-w-0 flex-1 truncate" title={v.value}>
                      {text(v.value)}
                    </span>
                    {on && <span className="sr-only">(selected)</span>}
                    <span className="text-xs tabular-nums text-muted-foreground">{formatCount(v.count)}</span>
                  </CommandItem>
                );
              })}
            </CommandList>
          </Command>
        )}
        {facet.more && mode === "select" && (
          <p className="px-gutter text-xs text-muted-foreground">
            The {FACET_VALUES_MAX} with the most {rows}
            {stored ? "; find another by your own id" : typed ? "; type one in to find another" : ""}.
          </p>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** A score's name and the range its value falls within, the list narrowed to
 *  the rows carrying one, as `?score=`, `score_min` and `score_max` narrow it. */
function ScoreSection({
  list,
  query,
  clearHref,
  onScore,
}: {
  list: ListName;
  query: ListQuery<string>;
  clearHref: string;
  onScore: (score: { score: string; score_min?: number; score_max?: number }) => void;
}) {
  const set = query.score !== undefined;
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? set;
  const [name, setName] = useState(query.score ?? "");
  const [least, setLeast] = useState(query.score_min === undefined ? "" : String(query.score_min));
  const [most, setMost] = useState(query.score_max === undefined ? "" : String(query.score_max));
  const bound = (text: string): number | undefined | null => {
    if (!text.trim()) return undefined;
    const value = Number(text.trim());
    return Number.isFinite(value) ? value : null;
  };
  const trimmed = name.trim();
  const [low, high] = [bound(least), bound(most)];
  const taken =
    trimmed !== "" &&
    [...trimmed].length <= SCORE_NAME_MAX &&
    low !== null &&
    high !== null &&
    (low === undefined || high === undefined || low <= high);
  return (
    <Collapsible open={open} onOpenChange={setChosen} className="border-b border-border">
      <div className="flex items-center gap-gutter">
        <CollapsibleTrigger asChild>
          <Button variant="ghost" size="sm" className="min-w-0 flex-1 justify-start">
            <ChevronRightIcon
              className={cn("text-muted-foreground transition-transform", open && "rotate-90")}
              aria-hidden="true"
            />
            <span className="truncate">Score</span>
            {set && (
              <Badge variant="secondary" className="tabular-nums">
                1
              </Badge>
            )}
          </Button>
        </CollapsibleTrigger>
        {set && (
          <Tooltip>
            <TooltipTrigger asChild>
              <Button asChild variant="ghost" size="icon">
                <Link href={clearHref} scroll={false} aria-label="Clear the score filter">
                  <XIcon />
                  <LinkPending />
                </Link>
              </Button>
            </TooltipTrigger>
            <TooltipContent>Clear</TooltipContent>
          </Tooltip>
        )}
      </div>
      <CollapsibleContent className="pb-gutter">
        <form
          className="grid gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (!taken) return;
            onScore({ score: trimmed, score_min: low ?? undefined, score_max: high ?? undefined });
          }}
        >
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="A score's name"
            aria-label="A score's name"
            autoComplete="off"
            spellCheck={false}
            className="text-xs"
          />
          <div className="flex gap-2">
            <Input
              value={least}
              onChange={(event) => setLeast(decimalTyped(event.target.value, true))}
              placeholder="At least"
              aria-label="At least"
              inputMode="decimal"
              autoComplete="off"
              className="text-xs"
            />
            <Input
              value={most}
              onChange={(event) => setMost(decimalTyped(event.target.value, true))}
              placeholder="At most"
              aria-label="At most"
              inputMode="decimal"
              autoComplete="off"
              className="text-xs"
            />
          </div>
          <p className="text-xs text-muted-foreground">{SCORE_FILTER_HINT[list]}</p>
          <Button type="submit" variant="outline" disabled={!taken}>
            Apply
          </Button>
        </form>
      </CollapsibleContent>
    </Collapsible>
  );
}

/** A user's or a session's id typed as the application sent it: the server
 *  answers what it is stored as, and the list is narrowed to that. Sent as
 *  typed, since ingest hashed it byte for byte; in the action's body, never
 *  the URL. */
function StoredId({
  filter,
  label,
  projectId,
  ticked,
  onAdd,
}: {
  filter: StoredIdFilter;
  label: string;
  projectId: string;
  ticked: readonly string[];
  onAdd: (value: string) => void;
}) {
  const { pending, run } = useToastAction();
  const [value, setValue] = useState("");
  const asked = `The ${label.toLowerCase()}'s id, as your app sends it`;
  const taken = value !== "" && fits(filter, value);
  return (
    <form
      className="grid gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (!taken || pending) return;
        run(
          () => storedIdAction(projectId, filter, value),
          ({ storedId }) => {
            setValue("");
            // Ticked already, it stays ticked: adding it again would untick it.
            if (!ticked.includes(storedId)) onAdd(storedId);
          },
        );
      }}
    >
      <div className="flex gap-2">
        <Input
          value={value}
          onChange={(event) => setValue(event.target.value)}
          disabled={pending}
          // Short enough to read whole in the panel at its own width.
          placeholder="The id your app sends"
          aria-label={asked}
          autoComplete="off"
          spellCheck={false}
          className="text-xs"
        />
        <Button type="submit" variant="outline" disabled={!taken || pending}>
          Find
        </Button>
      </div>
      <p className="px-gutter text-xs text-muted-foreground">
        Hashed as it is stored, then matched: no page shows the id itself.
      </p>
    </form>
  );
}

/** A tag typed in, as `key:value`, added to the filter as the list is
 *  narrowed to it. */
function TypedTag({ onAdd }: { onAdd: (value: string) => void }) {
  const [value, setValue] = useState("");
  const trimmed = value.trim();
  const asked = "A tag, as key:value";
  const taken = fits("tag", trimmed) && isTagPair(trimmed);
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
        className="text-xs"
      />
      <Button type="submit" variant="outline" disabled={!taken}>
        Add
      </Button>
    </form>
  );
}
