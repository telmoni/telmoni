"use client";

import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import {
  ArrowDown,
  ArrowUp,
  ListTree,
  Maximize2,
  Minimize2,
  SquareArrowOutUpRight,
} from "lucide-react";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";

import { useConsoleShell } from "@/components/console-shell";
import { CopyButton } from "@/components/copy-button";
import { PanelAction, SidePanel, SidePanelHeader } from "@/components/side-panel";
import { PanelEdge, usePanelWidth } from "@/components/traces/panel-width";
import { TraceBody, TraceMeta } from "@/components/traces/trace-view";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { NEXT_KEY, PREVIOUS_KEY } from "@/lib/keys";
import {
  closePeek,
  followEntry,
  keepRead,
  openPeek,
  peekedRun,
  pushedEntry,
  shownRead,
  type KeptRead,
  type TraceRead,
} from "@/lib/peek";
import type { TraceDetail } from "@/lib/server/entities/traces";
import { summarizeTrace } from "@/lib/trace-tree";
import { PEEK_PARAM, PEEK_NUDGED, shortId, traceHref } from "@/lib/traces";
import { offerPageSteps } from "@/lib/use-page-steps";
import { cn } from "@/lib/utils";

// A trace opened from the list without leaving it, as Langfuse's peek opens
// one: the console's side panel (`side-panel.tsx`) over the list, which fills
// the content area at a press and goes back. The run it shows is the URL's
// `?peek=`, kept in the browser's history (`lib/peek.ts`), so a link holds it
// and Back shuts it. The list's filters, sorts and pages carry no `peek`, so
// they shut it as they move the list; a new window keeps it, the window
// picker carrying the whole query along.

/** How long a read of the spans may take before the panel says it failed. */
const READ_TIMEOUT_MS = 15_000;

/** The run the peek holds open, from the URL; `null` while it is shut. */
export function usePeekedRun(): string | null {
  return peekedRun(useSearchParams().get(PEEK_PARAM));
}

async function readTrace(url: string, signal: AbortSignal): Promise<TraceRead<TraceDetail>> {
  try {
    const res = await fetch(url, { signal, cache: "no-store", headers: { accept: "application/json" } });
    if (res.status === 401) return { kind: "signed-out" };
    if (res.status === 404) return { kind: "missing" };
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const trace = (await res.json()) as TraceDetail;
    return trace.spans.length > 0 ? { kind: "ok", trace } : { kind: "missing" };
  } catch {
    return { kind: "unavailable" };
  }
}

/** The run's spans, read when it is opened and again at each `version`: a
 *  retry, or its spans landing (`keepRead`, `shownRead` in `lib/peek.ts`). */
function useTraceRead(url: string, version: number) {
  const [last, setLast] = useState<KeptRead<TraceDetail> | null>(null);
  useEffect(() => {
    // One controller, aborted by its timer or by the next read: joining two
    // signals (`AbortSignal.any`) is newer than browsers Next supports.
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), READ_TIMEOUT_MS);
    let current = true;
    void readTrace(url, controller.signal).then((read) => {
      clearTimeout(timer);
      if (current) setLast((was) => keepRead(was, { url, version, read }));
    });
    return () => {
      current = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [url, version]);
  return shownRead(last, url, version);
}

type Props = {
  organization: string;
  slug: string;
  /** The project's id, which its reads are named by. */
  projectId: string;
  /** The page's runs in the list's order: what the ↑ and ↓ buttons, and `j`
   *  and `k`, step through. */
  runs: readonly string[];
};

export function TracePeek(props: Props) {
  const run = usePeekedRun();
  const { element } = useConsoleShell();
  if (!run || !element) return null;
  return createPortal(<PeekPanel {...props} run={run} />, element);
}

function PeekPanel({ organization, slug, projectId, run, runs }: Props & { run: string }) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { full, width, toggleFull, edge } = usePanelWidth(panel);
  const [version, setVersion] = useState(0);
  const { read, stale } = useTraceRead(
    `/api/v1/traces/${encodeURIComponent(run)}?project=${encodeURIComponent(projectId)}`,
    version,
  );
  const pathname = usePathname();
  const search = useSearchParams().toString();
  const at = runs.indexOf(run);
  const previous = at > 0 ? runs[at - 1]! : null;
  const next = at !== -1 && at + 1 < runs.length ? runs[at + 1]! : null;
  const trace = read?.kind === "ok" ? read.trace : null;
  // Summed once a trace, not at each draw: a drag of the panel's edge draws
  // it at every move of the pointer.
  const summary = useMemo(() => (trace ? summarizeTrace(trace.spans) : null), [trace]);
  // The run on show: the one read while the next is on its way.
  const shown = trace?.run_id ?? run;

  // The entry the peek pushed, by its address, as it stood when the panel
  // opened, then followed through each draw — a refresh's, an action's, the
  // list's move to another window (`followEntry`) — so shutting goes back
  // over it only while it is the one in front.
  const entry = useRef<string | null>(null);
  useLayoutEffect(() => {
    entry.current = pushedEntry();
  }, []);
  useEffect(() => {
    entry.current = followEntry(entry.current);
  });
  const close = () => closePeek(entry.current !== null);

  // The run open as the panel shuts, for the focus to go back to its row, as
  // the agent's window hands it back (`agent-panel.tsx`).
  const openRun = useRef(run);
  useEffect(() => {
    openRun.current = run;
  }, [run]);
  const openRow = () =>
    [...document.querySelectorAll<HTMLElement>("[data-peek-run]")].find(
      (link) => link.dataset.peekRun === openRun.current,
    );

  // A run still going is read again as its spans land.
  useEffect(() => {
    const again = () => setVersion((n) => n + 1);
    window.addEventListener(PEEK_NUDGED, again);
    return () => window.removeEventListener(PEEK_NUDGED, again);
  }, []);

  // `j` and `k` step as the ↓ and ↑ buttons do, while the panel is open. Each
  // press reads where the peek stands then, from the URL, so two in quick
  // succession step twice rather than both from where the panel was drawn.
  const listed = useRef(runs);
  useEffect(() => {
    listed.current = runs;
  }, [runs]);
  useEffect(() => {
    const step = (by: number) => () => {
      const now = new URLSearchParams(window.location.search).get(PEEK_PARAM) ?? "";
      const to = listed.current[listed.current.indexOf(now) + by];
      if (listed.current.includes(now) && to) openPeek(to);
    };
    return offerPageSteps({ noun: "trace", next: step(1), previous: step(-1) });
  }, []);

  // Another trace on show takes the place of the last, and with it whatever
  // in it had the focus: the panel takes it back, so Escape, `j` and `k`
  // still reach it. A row of the list clicked to switch it hands it over too,
  // as the first click did. The ↑ and ↓ buttons keep theirs.
  useLayoutEffect(() => {
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus({ preventScroll: true });
  }, [trace?.run_id]);

  return (
    <SidePanel
      panel={panel}
      titleId={titleId}
      onClose={close}
      full={full}
      width={width}
      returnFocus={openRow}
    >
      <PanelEdge edge={edge} />
      <SidePanelHeader
        icon={ListTree}
        titleId={titleId}
        flush
        title={summary ? summary.root.agent : "Trace"}
        beside={
          <>
            <span className="shrink-0 text-xs text-muted-foreground" title={shown}>
              {shortId(shown)}
            </span>
            <CopyButton value={shown} label="the run's id" />
          </>
        }
        actions={
          <>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:text-foreground"
                  asChild
                >
                  <Link href={traceHref(organization, slug, run)} aria-label="Open the trace's page">
                    <SquareArrowOutUpRight />
                  </Link>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Open the trace&rsquo;s page</TooltipContent>
            </Tooltip>
            <PanelAction
              label="Previous trace"
              letterKey={PREVIOUS_KEY}
              unavailable={!previous}
              onClick={() => previous && openPeek(previous)}
            >
              <ArrowUp />
            </PanelAction>
            <PanelAction
              label="Next trace"
              letterKey={NEXT_KEY}
              unavailable={!next}
              onClick={() => next && openPeek(next)}
            >
              <ArrowDown />
            </PanelAction>
            {/* Named as the agent's window names its own; a phone's width
                has the panel at full size already. */}
            <PanelAction
              label={full ? "Exit full size" : "Full size"}
              onClick={toggleFull}
              className="max-md:hidden"
            >
              {full ? <Minimize2 /> : <Maximize2 />}
            </PanelAction>
          </>
        }
        onClose={close}
      >
        {trace && <TraceMeta trace={trace} organization={organization} slug={slug} />}
      </SidePanelHeader>
      <div
        aria-busy={read === null || stale}
        className={cn("flex min-h-0 flex-1 flex-col", stale && "opacity-60 transition-opacity")}
      >
        {read === null ? (
          <Note>Reading the trace&hellip;</Note>
        ) : read.kind === "ok" ? (
          <TraceBody
            key={read.trace.run_id}
            trace={read.trace}
            organization={organization}
            slug={slug}
            className="min-h-0 flex-1"
          />
        ) : read.kind === "missing" ? (
          <Note title="No trace with this id in this project.">
            Its spans may have passed their retention, or the run belongs to another project.
          </Note>
        ) : read.kind === "forbidden" ? (
          <Note title="This project's traces are not yours to read.">
            Ask an owner of the project for a role that reads them.
          </Note>
        ) : read.kind === "signed-out" ? (
          <Note title="Your session has ended.">
            <a
              href={`/auth/login?returnTo=${encodeURIComponent(`${pathname}?${search}`)}`}
              className="underline underline-offset-4"
            >
              Sign in again
            </a>{" "}
            to read it.
          </Note>
        ) : (
          <Note title="The trace could not be read.">
            <button
              type="button"
              onClick={() => setVersion((n) => n + 1)}
              className="cursor-pointer underline underline-offset-4"
            >
              Try again
            </button>{" "}
            in a moment.
          </Note>
        )}
      </div>
    </SidePanel>
  );
}

/** What the panel says in the trace's place. */
function Note({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 p-3.5 text-sm">
      {title && <p className="font-medium">{title}</p>}
      <p className="text-muted-foreground">{children}</p>
    </div>
  );
}
