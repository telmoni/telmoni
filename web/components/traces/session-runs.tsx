"use client";

import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronRight, SquareArrowOutUpRight } from "lucide-react";
import Link from "next/link";

import { DataTable, THead, Td, Th } from "@/components/data-table";
import { cellClass, cellOf, titleOf } from "@/components/traces/cells";
import { TraceOutline } from "@/components/traces/trace-outline";
import { KindIcon } from "@/components/traces/trace-view";
import { useLayout, type LayoutPage } from "@/components/traces/use-layout";
import {
  spanTitle,
  tracesOpenedFirst,
  type LayoutOptions,
} from "@/lib/layout";
import type { WindowQuery } from "@/lib/list-query";
import type { PinnedSpan, SessionDetail, SessionTrace, TraceDetail } from "@/lib/server/data";
import {
  TRACE_COLUMNS,
  formatDuration,
  shortId,
  spanMillis,
  traceHref,
  tracesHref,
  type FilterKey,
} from "@/lib/traces";
import { LayoutDepth, SpanStatus } from "@/lib/types/enums";
import { offerPageSteps } from "@/lib/use-page-steps";
import { cn } from "@/lib/utils";

// A session's traces under the layout the page stands in
// (`components/traces/use-layout.ts`): a row per trace in the order they
// ran, with the layout's columns; opened, a trace shows its spans as far as
// the layout opens them, read as it opens through the trace list's own
// Route Handler, so a session of hundreds of traces costs only the ones
// opened; shut, the spans the layout pinned stand under it. `j` and `k` step
// the trace stood on, and `o` opens it, through the console's one key
// handler.

/** How long a read of a trace's spans may take before the row says it failed. */
const READ_TIMEOUT_MS = 15_000;

type Props = LayoutPage & {
  organization: string;
  slug: string;
  projectId: string;
  sessionId: string;
  traces: readonly SessionTrace[];
  /** The span names the page's traces were read with on the server. */
  readWith: readonly string[];
  window: WindowQuery;
};

export function SessionRuns(props: Props) {
  const { options } = useLayout(props);
  const pinned = usePinnedSpans(props, options.pinned);
  // A new depth or opening starts the rows afresh: which are open follows
  // the layout, and a read already made is kept across it.
  const [reads] = useState(() => new Map<string, TraceDetail>());
  return (
    <Runs
      key={`${options.depth}:${options.open}`}
      {...props}
      options={options}
      pinned={pinned}
      reads={reads}
    />
  );
}

/** The pinned spans under each trace, by run: the page's own, read with
 *  the session, while the layout pins what the page was read with; else
 *  read again through the session's Route Handler, as the browser's own
 *  layout may pin what the server could not know. */
function usePinnedSpans(
  { projectId, sessionId, traces, readWith, window: query }: Props,
  pinned: readonly string[],
): ReadonlyMap<string, readonly PinnedSpan[]> {
  const same =
    pinned.length === readWith.length && pinned.every((name) => readWith.includes(name));
  const [read, setRead] = useState<{ key: string; spans: Map<string, PinnedSpan[]> } | null>(
    null,
  );
  const key = [...pinned].sort().join("\u0000");
  // By the names, not the array: a saved layout's is built afresh each draw.
  const names = pinned.join("\u0000");
  useEffect(() => {
    if (same || names === "") return;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), READ_TIMEOUT_MS);
    const params = new URLSearchParams({ project: projectId });
    if (query.days !== undefined) params.set("days", String(query.days));
    if (query.limit !== undefined) params.set("limit", String(query.limit));
    for (const name of names.split("\u0000")) params.append("pinned", name);
    // The route's answer is the session as the server checked it; a shape
    // this build cannot read is left as nothing pinned.
    void fetch(`/api/v1/sessions/${encodeURIComponent(sessionId)}?${params}`, {
      signal: controller.signal,
      cache: "no-store",
      headers: { accept: "application/json" },
    })
      .then(async (res) => (res.ok ? ((await res.json()) as SessionDetail) : null))
      .then((detail) => {
        clearTimeout(timer);
        if (!detail || !Array.isArray(detail.traces)) return;
        const spans = new Map(detail.traces.map((t) => [t.run_id, t.pinned ?? []]));
        setRead({ key, spans });
      })
      .catch(() => clearTimeout(timer));
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [same, names, key, projectId, sessionId, query.days, query.limit]);

  if (pinned.length === 0) return new Map();
  if (same) return new Map(traces.map((t) => [t.run_id, t.pinned]));
  return read?.key === key ? read.spans : new Map();
}

function Runs({
  organization,
  slug,
  projectId,
  traces,
  window: query,
  options,
  pinned,
  reads,
}: Props & {
  options: LayoutOptions;
  pinned: ReadonlyMap<string, readonly PinnedSpan[]>;
  reads: Map<string, TraceDetail>;
}) {
  const columns = TRACE_COLUMNS.filter((c) => options.columns.includes(c.key));
  const openable = options.depth !== LayoutDepth.Trace;
  const [opened, setOpened] = useState<ReadonlySet<string>>(() =>
    tracesOpenedFirst(options, traces),
  );
  const [current, setCurrent] = useState<string | null>(null);
  const narrowedTo = (filter: FilterKey, value: string) =>
    tracesHref(organization, slug, { days: query.days }, { [filter]: [value] });

  const toggle = (run: string) => {
    if (!openable) return;
    setOpened((was) => {
      const next = new Set(was);
      if (next.has(run)) next.delete(run);
      else next.add(run);
      return next;
    });
  };

  // `j` and `k` step the trace stood on, from the first; `o` opens it. Each
  // press reads where the page stands then, so two in quick succession step
  // twice.
  const standing = useRef({ current, runs: traces.map((t) => t.run_id) });
  useEffect(() => {
    standing.current = { current, runs: traces.map((t) => t.run_id) };
  });
  useEffect(() => {
    const move = (by: number) => () => {
      const { current: at, runs } = standing.current;
      const i = at ? runs.indexOf(at) : -1;
      const to = runs[i === -1 ? (by > 0 ? 0 : runs.length - 1) : i + by];
      if (!to) return;
      setCurrent(to);
      document
        .querySelector<HTMLElement>(`[data-run="${CSS.escape(to)}"]`)
        ?.scrollIntoView?.({ block: "nearest" });
    };
    return offerPageSteps({
      noun: "trace",
      next: move(1),
      previous: move(-1),
      // Offered only where a trace opens at all: the sheet lists what works.
      open: openable
        ? () => {
            const { current: at } = standing.current;
            if (at) toggle(at);
          }
        : undefined,
    });
    // The depth is fixed for this mount (the rows are keyed on it), so
    // offering once is enough.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <DataTable>
      <THead>
        <Th className="w-0 pr-0" />
        <Th className="whitespace-nowrap">Trace</Th>
        {columns.map((column) => (
          <Th key={column.key} className={cn("whitespace-nowrap", column.numeric && "text-right")}>
            {column.label}
          </Th>
        ))}
      </THead>
      <tbody>
        {traces.map((trace) => {
          const run = trace.run_id;
          const open = openable && opened.has(run);
          const pins = pinned.get(run) ?? [];
          const stood = current === run;
          return (
            <Fragment key={run}>
              <tr
                data-run={run}
                aria-current={stood ? "true" : undefined}
                onClick={(event) => {
                  if ((event.target as HTMLElement).closest("a, button, input, label")) return;
                  if (window.getSelection()?.toString()) return;
                  setCurrent(run);
                  toggle(run);
                }}
                className={cn(openable && "cursor-pointer", stood ? "bg-muted" : "hover:bg-muted/50")}
              >
                <Td className="pr-0">
                  {openable ? (
                    <button
                      type="button"
                      aria-label={open ? "Shut the trace" : "Open the trace"}
                      aria-expanded={open}
                      onClick={() => {
                        setCurrent(run);
                        toggle(run);
                      }}
                      className="flex size-5 cursor-pointer items-center justify-center text-muted-foreground hover:text-foreground"
                    >
                      <ChevronRight className={cn("size-4 transition-transform", open && "rotate-90")} />
                    </button>
                  ) : (
                    <span className="inline-block size-5" aria-hidden="true" />
                  )}
                </Td>
                <Td className="whitespace-nowrap">
                  <Link
                    href={traceHref(organization, slug, run)}
                    className="font-mono text-xs underline-offset-4 hover:underline"
                  >
                    {shortId(run)}
                  </Link>
                </Td>
                {columns.map((column) => (
                  <Td key={column.key} className={cellClass(column)} title={titleOf(column, trace)}>
                    {cellOf(column, trace, { days: query.days }, narrowedTo)}
                  </Td>
                ))}
              </tr>
              {(open || pins.length > 0) && (
                <tr data-run-under={run}>
                  <td colSpan={columns.length + 2} className="bg-muted/30 px-4 py-2">
                    {open ? (
                      <OpenedTrace
                        run={run}
                        projectId={projectId}
                        options={options}
                        reads={reads}
                        page={traceHref(organization, slug, run)}
                      />
                    ) : (
                      <PinnedStrip spans={pins} />
                    )}
                  </td>
                </tr>
              )}
            </Fragment>
          );
        })}
      </tbody>
    </DataTable>
  );
}

/** The spans the layout pinned, under a trace that is shut. */
function PinnedStrip({ spans }: { spans: readonly PinnedSpan[] }) {
  return (
    <ul className="flex flex-wrap gap-1.5" aria-label="Pinned spans">
      {spans.map((span) => (
        <li
          key={span.span_id}
          className="inline-flex max-w-full items-center gap-1.5 rounded-md border border-border bg-background px-2 py-0.5 text-xs"
          title={span.name}
        >
          <KindIcon span={span} className="size-3.5" />
          <span className="truncate">{spanTitle(span)}</span>
          <span className="tabular-nums text-muted-foreground">{formatDuration(spanMillis(span))}</span>
          {span.status === SpanStatus.Error && (
            <span className="text-destructive">{span.error_type || "Error"}</span>
          )}
        </li>
      ))}
    </ul>
  );
}

type Read =
  | { kind: "ok"; trace: TraceDetail }
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" }
  | { kind: "signed-out" };

/** A trace opened: its spans read once, through the trace list's own route,
 *  and drawn as far as the layout opens them. A run still going is read
 *  again each time it is opened; one that has ended is kept. */
function OpenedTrace({
  run,
  projectId,
  options,
  reads,
  page,
}: {
  run: string;
  projectId: string;
  options: LayoutOptions;
  reads: Map<string, TraceDetail>;
  page: string;
}) {
  const kept = reads.get(run);
  const [read, setRead] = useState<Read | null>(kept ? { kind: "ok", trace: kept } : null);
  const [tries, setTries] = useState(0);
  useEffect(() => {
    if (reads.has(run) && tries === 0) return;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), READ_TIMEOUT_MS);
    let current = true;
    const url = `/api/v1/traces/${encodeURIComponent(run)}?project=${encodeURIComponent(projectId)}`;
    void fetch(url, { signal: controller.signal, cache: "no-store", headers: { accept: "application/json" } })
      .then(async (res): Promise<Read> => {
        if (res.status === 401) return { kind: "signed-out" };
        if (res.status === 404) return { kind: "missing" };
        if (res.status === 403) return { kind: "forbidden" };
        if (!res.ok) return { kind: "unavailable" };
        const trace = (await res.json()) as TraceDetail;
        if (trace.spans.length === 0) return { kind: "missing" };
        if (trace.spans.some((s) => s.kind === "run")) reads.set(run, trace);
        return { kind: "ok", trace };
      })
      .catch((): Read => ({ kind: "unavailable" }))
      .then((result) => {
        clearTimeout(timer);
        if (current) setRead(result);
      });
    return () => {
      current = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [run, projectId, reads, tries]);

  return (
    <div className="grid gap-1">
      <div className="flex items-center justify-end">
        <Link
          href={page}
          className="inline-flex items-center gap-1 text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
        >
          Open the trace&rsquo;s page
          <SquareArrowOutUpRight className="size-3" aria-hidden="true" />
        </Link>
      </div>
      {read === null ? (
        <Note>Reading the trace&hellip;</Note>
      ) : read.kind === "ok" ? (
        <TraceOutline trace={read.trace} options={options} />
      ) : read.kind === "missing" ? (
        <Note>Its spans have passed their retention, or the run belongs to another project.</Note>
      ) : read.kind === "forbidden" ? (
        <Note>This project&rsquo;s traces are not yours to read.</Note>
      ) : read.kind === "signed-out" ? (
        <Note>Your session has ended. Sign in again to read it.</Note>
      ) : (
        <Note>
          The trace could not be read.{" "}
          <button
            type="button"
            onClick={() => setTries((n) => n + 1)}
            className="cursor-pointer underline underline-offset-4"
          >
            Try again
          </button>
          .
        </Note>
      )}
    </div>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <p className="px-2 py-1.5 text-xs text-muted-foreground">{children}</p>;
}
