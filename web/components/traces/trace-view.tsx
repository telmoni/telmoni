"use client";

import {
  memo,
  useMemo,
  useReducer,
  useRef,
  useState,
  useSyncExternalStore,
  type Dispatch,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
} from "react";
import { ArrowUpRight, ChevronRight, PanelLeft, SearchIcon } from "lucide-react";
import Link from "next/link";

import { LocalTime } from "@/components/local-time";
import { StatusBadge } from "@/components/status-badge";
import {
  INDENT_MAX,
  KindIcon,
  PinnedChip,
  SpanBar,
  SpanFacts,
  SpanGuides,
  TruncatedNote,
  indentStyle,
} from "@/components/traces/span-parts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  OPEN_ALL_UP_TO,
  PINNED_PER_TRACE,
  openingFor,
  shutBranches,
  visibleTree,
  type LayoutOptions,
} from "@/lib/layout";
import { incidentHref } from "@/lib/alerts";
import { subscribeNever } from "@/lib/in-browser";
import { windowReaching } from "@/lib/list-query";
import type { Incident } from "@/lib/server/entities/alerts";
import type { RunSpend, TraceDetail, TraceSpan } from "@/lib/server/entities/traces";
import { sessionHref } from "@/lib/sessions";
import {
  branchesFrom,
  matchingRows,
  parentRow,
  spanTree,
  summarizeTrace,
  treeReducer,
  treeRows,
  type SpanNode,
  type TraceSummary,
  type TreeAction,
} from "@/lib/trace-tree";
import {
  KIND_LABEL,
  formatCost,
  formatCount,
  formatDuration,
  barsById,
  shortId,
  spanMillis,
  tracesHref,
} from "@/lib/traces";
import { SpanKind, SpanStatus } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

// One trace read a span at a time: its
// spans as the tree they ran in down one side, and the span chosen there, in
// full, beside it. Every pixel is metadata: no prompt, completion or tool
// payload reaches the console, in any content mode, and a span's attributes
// are what ingest kept as metadata under their own names.

/** How a span ended, the error's class beside it where it failed. */
function SpanStatusBadge({ span }: { span: Pick<TraceSpan, "status" | "error_type"> }) {
  return <StatusBadge status={span.status} detail={span.error_type} />;
}

/** A value with a page of its own, linked there, with the arrow that says
 *  the link leaves the trace. */
function MetaLink({ href, title, children }: { href: string; title?: string; children: ReactNode }) {
  return (
    <Link
      href={href}
      title={title}
      className="inline-flex min-w-0 items-center gap-0.5 underline-offset-4 hover:text-foreground hover:underline"
    >
      <span className="truncate">{children}</span>
      <ArrowUpRight className="size-3 shrink-0" aria-hidden="true" />
    </Link>
  );
}


/** The window of days a link to the list, a user's traces or a session's
 *  page needs to hold a run that started at `iso`: the pages open on a week.
 *  None on the server and in the browser's first pass, the browser's own
 *  clock after, so the two draw one link — React keeps the server's
 *  attribute where they differ, and the two clocks may sit either side of
 *  a window's edge. */
function useWindowReaching(iso: string): number | undefined {
  return useSyncExternalStore(subscribeNever, () => windowReaching(iso), () => undefined);
}

/** What the run spent, each call priced as an alert on its cost prices it,
 *  and how many of the models it called no price covers, named on hover. */
function RunSpent({ spend }: { spend: RunSpend }) {
  const unpriced = spend.unpriced.length;
  return (
    <span
      className="tabular-nums"
      title={
        unpriced > 0
          ? `No price for ${spend.unpriced.map((name) => `${name.provider} ${name.model}`).join(", ")}`
          : "Each call's cost as reported, else its tokens at Telmoni's price"
      }
    >
      {formatCost(spend.spent)}
      {unpriced > 0 && `, ${formatCount(unpriced)} ${unpriced === 1 ? "model" : "models"} unpriced`}
    </span>
  );
}

/** The trace summed on one line under its name: how it ended, when it began,
 *  how long it ran, its tokens, what it spent as its cost alert reads it, the
 *  session, the user and the tags it carried, and on its page the incidents
 *  whose notices showed it. Drawn again only for another trace (`memo`): the
 *  peek's edge, dragged, draws the panel around it at every move. */
export const TraceMeta = memo(function TraceMeta({
  trace,
  organization,
  slug,
  incidents = [],
}: {
  trace: TraceDetail;
  organization: string;
  slug: string;
  incidents?: readonly Pick<Incident, "id" | "rule_name">[];
}) {
  const summary = useMemo(() => summarizeTrace(trace.spans), [trace.spans]);
  const { run, root } = summary;
  const reaching = useWindowReaching(summary.startedAt);
  const tokens = summary.inputTokens + summary.outputTokens;
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
      <StatusBadge status={run?.status ?? null} detail={run?.error_type} />
      <LocalTime iso={summary.startedAt} />
      <span className="tabular-nums">{formatDuration(summary.durationMs)}</span>
      <span
        className="tabular-nums"
        title={`${formatCount(summary.inputTokens)} in, ${formatCount(summary.outputTokens)} out`}
      >
        {formatCount(tokens)} {tokens === 1 ? "token" : "tokens"}
      </span>
      {trace.spend && (trace.spend.spent > 0 || trace.spend.unpriced.length > 0) && (
        <RunSpent spend={trace.spend} />
      )}
      {root.session_id && (
        <MetaLink
          href={sessionHref(organization, slug, root.session_id, { days: reaching })}
          title={`Session ${root.session_id}`}
        >
          session
        </MetaLink>
      )}
      {root.user_id && (
        <MetaLink
          href={tracesHref(organization, slug, { user: [root.user_id], days: reaching })}
          title={`Every trace of user ${root.user_id}`}
        >
          user {shortId(root.user_id)}
        </MetaLink>
      )}
      {Object.entries(root.tags).map(([key, value]) => (
        <Badge key={key} variant="secondary" className="text-xs font-normal">
          {key}={value}
        </Badge>
      ))}
      {incidents.map((incident) => (
        <MetaLink
          key={incident.id}
          href={incidentHref(organization, slug, incident.id)}
          title={`The incident of ${incident.rule_name} whose notice showed this run`}
        >
          incident: {incident.rule_name}
        </MetaLink>
      ))}
    </div>
  );
});

/** The button that hides the spans' pane, or shows it again from the span's
 *  heading, as the filters' sidebar does. Each goes as it is pressed, so the
 *  focus moves to the other (`TraceBody`). */
function SpansButton({
  ref,
  shown,
  onClick,
}: {
  ref: Ref<HTMLButtonElement>;
  shown: boolean;
  onClick: () => void;
}) {
  const label = shown ? "Hide the spans" : "Show the spans";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          ref={ref}
          variant="ghost"
          size="icon"
          className="text-muted-foreground hover:text-foreground"
          aria-label={label}
          aria-expanded={shown}
          onClick={onClick}
        >
          <PanelLeft />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** The trace's spans and the one chosen among them, side by side; under each
 *  other on a phone. The run's own span is chosen first. Under a `layout`
 *  (`lib/layout.ts`) the tree shows what the depth and the filter admit,
 *  starts open as the layout opens it, with its timeline on or off, and the
 *  spans the layout pinned stand above it; the caller keys the body on the
 *  layout, so a change starts it afresh. Drawn again only for another trace
 *  (`memo`), as the line above it is. */
export const TraceBody = memo(function TraceBody({
  trace,
  organization,
  slug,
  layout,
  className,
}: {
  trace: TraceDetail;
  organization: string;
  slug: string;
  layout?: LayoutOptions;
  className?: string;
}) {
  const nodes = useMemo(
    () => (layout ? visibleTree(spanTree(trace.spans), layout) : spanTree(trace.spans)),
    [trace.spans, layout],
  );
  const summary = useMemo(() => summarizeTrace(trace.spans), [trace.spans]);
  const bars = useMemo(() => barsById(trace.spans), [trace.spans]);
  const parentOf = useMemo(
    () => new Map(trace.spans.map((s) => [s.span_id, s.parent_span_id])),
    [trace.spans],
  );
  const pinned = useMemo(
    () =>
      layout
        ? trace.spans.filter((s) => layout.pinned.includes(s.name)).slice(0, PINNED_PER_TRACE)
        : [],
    [trace.spans, layout],
  );
  // Past OPEN_ALL_UP_TO spans a trace opens with its branches shut but the
  // run's own, whatever the layout says, so its first rows draw at once; a
  // search, or a branch opened, finds the rest.
  const [{ chosen, shut, parentOf: followed }, dispatch] = useReducer(treeReducer, null, () => ({
    chosen: summary.root.span_id,
    shut: layout
      ? shutBranches(nodes, openingFor(trace.spans.length, layout.open))
      : trace.spans.length > OPEN_ALL_UP_TO
        ? branchesFrom(nodes, 1)
        : new Set<string>(),
    parentOf,
  }));
  // A run still going is read again as its spans land: the tree follows them.
  if (followed !== parentOf) dispatch({ type: "spans", parentOf });
  const [search, setSearch] = useState("");
  const [timeline, setTimeline] = useState(layout?.timeline ?? false);
  const [spansShown, setSpansShown] = useState(true);
  const tree = useRef<HTMLDivElement>(null);
  const hideSpans = useRef<HTMLButtonElement>(null);
  const showSpans = useRef<HTMLButtonElement>(null);

  const searching = search.trim() !== "";
  const rows = searching ? matchingRows(nodes, search) : treeRows(nodes, shut);
  const span = trace.spans.find((s) => s.span_id === chosen) ?? summary.root;
  // The one row Tab lands on, as a tree has: the chosen span's, or the first
  // row while a search hides it.
  const tabStop = rows.some((r) => r.span.span_id === span.span_id)
    ? span.span_id
    : rows[0]?.span.span_id;

  // Chosen from the keyboard, the row takes the focus too, once it is drawn:
  // a row a shut branch hid is not in the tree until then.
  const chooseAndFocus = (id: string) => {
    dispatch({ type: "choose", id });
    requestAnimationFrame(() =>
      [...(tree.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [])]
        .find((row) => row.dataset.span === id)
        ?.focus(),
    );
  };

  // The keys a tree answers (WAI-ARIA's tree view): up and down a row, Home
  // and End, right to open a branch or step into it, left to shut it or step
  // out to the span above. The span chosen follows the focus, as the pane
  // beside it is there to read whichever row stands chosen. From the row the
  // focus is on: a chevron clicked focuses its row without choosing it.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const from =
      (e.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]')?.dataset.span ?? tabStop;
    const at = rows.findIndex((r) => r.span.span_id === from);
    const node = rows[at];
    if (!node) return;
    const id = node.span.span_id;
    const branch = !searching && node.children.length > 0;
    let to: SpanNode | null | undefined;
    switch (e.key) {
      case "ArrowDown":
        to = rows[at + 1];
        break;
      case "ArrowUp":
        to = rows[at - 1];
        break;
      case "Home":
        to = rows[0];
        break;
      case "End":
        to = rows[rows.length - 1];
        break;
      case "ArrowRight":
        if (branch && shut.has(id)) {
          e.preventDefault();
          dispatch({ type: "toggle", id });
          return;
        }
        to = branch ? node.children[0] : undefined;
        break;
      case "ArrowLeft":
        if (branch && !shut.has(id)) {
          e.preventDefault();
          dispatch({ type: "toggle", id });
          return;
        }
        to = parentRow(rows, node);
        break;
      default:
        return;
    }
    e.preventDefault();
    if (to) chooseAndFocus(to.span.span_id);
  };

  return (
    <div className={cn("flex min-h-0 flex-col md:flex-row", className)}>
      {spansShown && (
        <div className="flex max-h-[45%] min-h-0 shrink-0 flex-col border-b border-border md:max-h-none md:w-2/5 md:max-w-sm md:min-w-60 md:border-r md:border-b-0">
          <div className="flex shrink-0 items-center gap-2 px-gutter pt-gutter pb-2">
            <SpansButton
              ref={hideSpans}
              shown
              onClick={() => {
                setSpansShown(false);
                requestAnimationFrame(() => showSpans.current?.focus());
              }}
            />
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={timeline ? "timeline" : "tree"}
              onValueChange={(next) => next && setTimeline(next === "timeline")}
              aria-label="How the spans are laid out"
              className="ml-auto"
            >
              <ToggleGroupItem value="tree">Tree</ToggleGroupItem>
              <ToggleGroupItem value="timeline">Timeline</ToggleGroupItem>
            </ToggleGroup>
          </div>
          {pinned.length > 0 && (
            <div className="flex shrink-0 flex-wrap gap-1.5 px-gutter pb-2" aria-label="Pinned spans">
              {pinned.map((span) => (
                <button
                  key={span.span_id}
                  type="button"
                  onClick={() => chooseAndFocus(span.span_id)}
                  aria-pressed={span.span_id === chosen}
                  className={cn(
                    "inline-flex max-w-full cursor-pointer items-center gap-1.5 rounded-md border border-border px-2 py-0.5 text-xs hover:bg-muted/50",
                    span.span_id === chosen && "bg-muted",
                  )}
                >
                  <PinnedChip span={span} />
                </button>
              ))}
            </div>
          )}
          <div className="relative shrink-0 px-gutter pb-2">
            <SearchIcon
              className="pointer-events-none absolute top-1/2 left-6 size-4 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <Input
              type="search"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              // Escape empties the search first, as a search field's own
              // does, and shuts the peek only once there is nothing to empty.
              onKeyDown={(event) => {
                if (event.key === "Escape" && search) {
                  event.preventDefault();
                  setSearch("");
                }
              }}
              placeholder="Search spans"
              aria-label="Search spans"
              className="pl-8"
            />
          </div>
          {rows.length === 0 ? (
            <p className="px-gutter py-2 text-xs text-muted-foreground">No span matches.</p>
          ) : (
            <div
              ref={tree}
              role="tree"
              aria-label="Spans"
              onKeyDown={onKeyDown}
              className="min-h-0 flex-1 overflow-y-auto px-2 pb-gutter"
            >
              {rows.map((node) => {
                const id = node.span.span_id;
                const bar = bars.get(id);
                return (
                  <SpanRow
                    key={id}
                    node={node}
                    chosen={id === span.span_id}
                    tabStop={id === tabStop}
                    branch={!searching && node.children.length > 0}
                    open={!shut.has(id)}
                    bar={timeline && bar ? bar : null}
                    dispatch={dispatch}
                  />
                );
              })}
            </div>
          )}
          <TruncatedNote trace={trace} className="shrink-0 border-t border-border px-gutter py-2" />
        </div>
      )}
      <SpanDetail
        span={span}
        summary={summary}
        runId={trace.run_id}
        organization={organization}
        slug={slug}
        spansShown={spansShown}
        showSpansRef={showSpans}
        onShowSpans={() => {
          setSpansShown(true);
          requestAnimationFrame(() => hideSpans.current?.focus());
        }}
        className="min-h-0 min-w-0 flex-1"
      />
    </div>
  );
});

/** One span's row: its kind, its name, how long it ran — and its tokens, for a
 *  model call — under the spans it ran inside, a guide standing for each.
 *  Drawn again only when its own state moves (`memo`): the one `dispatch`
 *  never does, so a run's thousands of rows hold as another is chosen. */
const SpanRow = memo(function SpanRow({
  node,
  chosen,
  tabStop,
  branch,
  open,
  bar,
  dispatch,
}: {
  node: SpanNode;
  chosen: boolean;
  tabStop: boolean;
  branch: boolean;
  open: boolean;
  bar: { offset: number; width: number } | null;
  dispatch: Dispatch<TreeAction>;
}) {
  const { span } = node;
  const depth = Math.min(node.depth, INDENT_MAX);
  const failed = span.status === SpanStatus.Error;
  return (
    <div
      role="treeitem"
      aria-level={node.depth + 1}
      aria-setsize={node.siblings}
      aria-posinset={node.position}
      aria-selected={chosen}
      aria-expanded={branch ? open : undefined}
      tabIndex={tabStop ? 0 : -1}
      data-span={span.span_id}
      onClick={() => dispatch({ type: "choose", id: span.span_id })}
      className={cn(
        "relative flex cursor-pointer items-start gap-1.5 rounded-md py-1.5 pr-2 outline-none",
        bar && "pb-5",
        chosen ? "bg-muted" : "hover:bg-muted/50",
      )}
      style={indentStyle(depth)}
    >
      <SpanGuides depth={depth} />
      {/* The chevron is the mouse's way to open a branch; the keyboard's is
          the tree's own arrows, so it is not a control of its own. */}
      <span
        aria-hidden="true"
        className="flex h-5 w-4 shrink-0 items-center justify-center"
        onClick={
          branch
            ? (event) => {
                event.stopPropagation();
                dispatch({ type: "toggle", id: span.span_id });
              }
            : undefined
        }
      >
        {branch && (
          <ChevronRight
            className={cn("size-3.5 text-muted-foreground transition-transform", open && "rotate-90")}
          />
        )}
      </span>
      <KindIcon span={span} className="mt-0.5" />
      <span className="grid min-w-0 flex-1 gap-0.5">
        <span className="truncate text-sm" title={span.name}>
          {span.name}
        </span>
        <span className="flex flex-wrap gap-x-2">
          <SpanFacts span={span} />
        </span>
      </span>
      {bar && <SpanBar bar={bar} failed={failed} className="bottom-2" />}
    </div>
  );
});

/** One term and its value in the span's details: an id, or an attribute's
 *  name and value. */
function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(0,9rem)_minmax(0,1fr)] gap-3 border-b border-border py-2 last:border-b-0">
      <dt className="min-w-0 wrap-break-word text-muted-foreground">{label}</dt>
      <dd className="min-w-0 wrap-break-word">{children}</dd>
    </div>
  );
}

/** A field whose value narrows a list to it. */
function FilterField({ label, href, children }: { label: string; href: string; children: ReactNode }) {
  return (
    <Field label={label}>
      <Link href={href} className="underline-offset-4 hover:underline">
        {children}
      </Link>
    </Field>
  );
}

/** A value as the server sent it. */
function Raw({ value }: { value: unknown }) {
  return (
    <pre className="overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs leading-5 break-all whitespace-pre-wrap">
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}

/** The span chosen, in full: what it was, how it ended and when, what it
 *  spent, and every attribute kept with it; formatted, or raw as the server
 *  sent it. The tab and the format hold as another span is chosen. */
function SpanDetail({
  span,
  summary,
  runId,
  organization,
  slug,
  spansShown,
  showSpansRef,
  onShowSpans,
  className,
}: {
  span: TraceSpan;
  /** The whole trace's sums, which the run's own span is read with. */
  summary: TraceSummary;
  runId: string;
  organization: string;
  slug: string;
  spansShown: boolean;
  showSpansRef: Ref<HTMLButtonElement>;
  onShowSpans: () => void;
  className?: string;
}) {
  const [tab, setTab] = useState("details");
  const [raw, setRaw] = useState(false);
  const attributes = Object.entries(span.attributes).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const reaching = useWindowReaching(span.started_at);
  const tokens = span.input_tokens + span.output_tokens;
  const run = span.kind === SpanKind.Run;
  const tags = Object.entries(span.tags);

  return (
    <section aria-label="The span chosen" className={cn("flex flex-col", className)}>
      <div className="grid shrink-0 gap-1.5 border-b border-border p-gutter">
        <div className="flex min-h-9 min-w-0 items-center gap-2">
          {!spansShown && <SpansButton ref={showSpansRef} shown={false} onClick={onShowSpans} />}
          <KindIcon span={span} />
          <h3 className="min-w-0 truncate text-sm font-medium" title={span.name}>
            {span.name}
          </h3>
          <span className="shrink-0 text-xs text-muted-foreground">{KIND_LABEL[span.kind]}</span>
        </div>
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs tabular-nums text-muted-foreground">
          <SpanStatusBadge span={span} />
          <LocalTime iso={span.started_at} mode="precise" />
          <span>{formatDuration(spanMillis(span))}</span>
          {span.kind === SpanKind.ModelCall && (
            <span title={`${formatCount(span.input_tokens)} in, ${formatCount(span.output_tokens)} out`}>
              {formatCount(tokens)} tokens
            </span>
          )}
          {span.model && (
            <MetaLink
              href={tracesHref(organization, slug, { model: [span.model], days: reaching })}
              title={`Every trace that called ${span.model}`}
            >
              {span.model}
            </MetaLink>
          )}
        </div>
      </div>
      <Tabs value={tab} onValueChange={setTab} className="min-h-0 flex-1 gap-0">
        <div className="flex shrink-0 items-center gap-2 border-b border-border px-gutter py-1.5">
          <TabsList variant="line">
            <TabsTrigger value="details">Details</TabsTrigger>
            <TabsTrigger value="attributes">
              Attributes
              {attributes.length > 0 && (
                <span className="tabular-nums text-muted-foreground">{attributes.length}</span>
              )}
            </TabsTrigger>
          </TabsList>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            value={raw ? "raw" : "formatted"}
            onValueChange={(next) => next && setRaw(next === "raw")}
            aria-label="How the span is shown"
            className="ml-auto"
          >
            <ToggleGroupItem value="formatted">Formatted</ToggleGroupItem>
            <ToggleGroupItem value="raw">Raw</ToggleGroupItem>
          </ToggleGroup>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-gutter py-2">
          <TabsContent value="details">
            {raw ? (
              <Raw value={span} />
            ) : (
              <dl className="text-sm">
                <Field label="Kind">{KIND_LABEL[span.kind]}</Field>
                <Field label="Status">
                  <SpanStatusBadge span={span} />
                </Field>
                {run && <Field label="Agent">{span.agent}</Field>}
                {run && (
                  <Field label="Calls">
                    {formatCount(summary.steps)} {summary.steps === 1 ? "step" : "steps"},{" "}
                    {formatCount(summary.modelCalls + summary.toolCalls)}{" "}
                    {summary.modelCalls + summary.toolCalls === 1 ? "call" : "calls"}:{" "}
                    <span className="text-muted-foreground">
                      {formatCount(summary.modelCalls)} to a model, {formatCount(summary.toolCalls)} to a tool
                    </span>
                  </Field>
                )}
                {run && (
                  <Field label="Tokens">
                    {formatCount(summary.inputTokens + summary.outputTokens)}{" "}
                    <span className="text-muted-foreground">
                      ({formatCount(summary.inputTokens)} in, {formatCount(summary.outputTokens)} out)
                    </span>
                  </Field>
                )}
                {span.model && (
                  <FilterField
                    label="Model"
                    href={tracesHref(organization, slug, { model: [span.model], days: reaching })}
                  >
                    {span.model}
                  </FilterField>
                )}
                {span.provider && <Field label="Provider">{span.provider}</Field>}
                {span.environment && (
                  <FilterField
                    label="Environment"
                    href={tracesHref(organization, slug, { environment: [span.environment], days: reaching })}
                  >
                    {span.environment}
                  </FilterField>
                )}
                {span.release && (
                  <FilterField
                    label="Release"
                    href={tracesHref(organization, slug, { release: [span.release], days: reaching })}
                  >
                    {span.release}
                  </FilterField>
                )}
                {span.kind === SpanKind.ToolCall && (
                  <FilterField
                    label="Tool"
                    href={tracesHref(organization, slug, { tool: [span.name], days: reaching })}
                  >
                    {span.name}
                  </FilterField>
                )}
                {(span.kind === SpanKind.ModelCall || tokens > 0) && (
                  <>
                    <Field label="Input tokens">{formatCount(span.input_tokens)}</Field>
                    <Field label="Output tokens">{formatCount(span.output_tokens)}</Field>
                    <Field label="Cache read">{formatCount(span.cache_read_input_tokens)}</Field>
                    <Field label="Cache write">{formatCount(span.cache_write_input_tokens)}</Field>
                  </>
                )}
                {span.cost !== null && <Field label="Cost">{formatCost(span.cost)}</Field>}
                <Field label="Started">
                  <LocalTime iso={span.started_at} mode="precise" />
                </Field>
                <Field label="Ended">
                  <LocalTime iso={span.ended_at} mode="precise" />
                </Field>
                <Field label="Duration">{formatDuration(spanMillis(span))}</Field>
                {run && span.user_id && (
                  <FilterField
                    label="User"
                    href={tracesHref(organization, slug, { user: [span.user_id], days: reaching })}
                  >
                    {span.user_id}
                  </FilterField>
                )}
                {run && span.session_id && (
                  <FilterField
                    label="Session"
                    href={sessionHref(organization, slug, span.session_id, { days: reaching })}
                  >
                    {span.session_id}
                  </FilterField>
                )}
                {tags.length > 0 && (
                  <Field label="Tags">
                    <span className="flex flex-wrap gap-1">
                      {tags.map(([key, value]) => (
                        <Badge key={key} variant="secondary" className="text-xs font-normal">
                          {key}={value}
                        </Badge>
                      ))}
                    </span>
                  </Field>
                )}
                {span.sample_rate !== null && span.sample_rate !== 1 && (
                  <Field label="Sample rate">{span.sample_rate}</Field>
                )}
                {run && (
                  <Field label="Run id">
                    {runId}
                  </Field>
                )}
                <Field label="Span id">
                  {span.span_id}
                </Field>
                {span.parent_span_id && (
                  <Field label="Parent span id">
                    {span.parent_span_id}
                  </Field>
                )}
                <Field label="Trace id">
                  {span.trace_id}
                </Field>
                <Field label="Received">
                  <LocalTime iso={span.received_at} mode="precise" />
                </Field>
              </dl>
            )}
          </TabsContent>
          <TabsContent value="attributes">
            {attributes.length === 0 ? (
              <p className="py-2 text-sm text-muted-foreground">
                No other attributes were kept with this span.
              </p>
            ) : raw ? (
              <Raw value={span.attributes} />
            ) : (
              <dl className="text-sm">
                {attributes.map(([key, value]) => (
                  <Field key={key} label={key}>
                    {value}
                  </Field>
                ))}
              </dl>
            )}
          </TabsContent>
        </div>
      </Tabs>
    </section>
  );
}
