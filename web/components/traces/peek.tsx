"use client";

import { useEffect, useEffectEvent, useId, useLayoutEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { ListTree } from "lucide-react";
import { useSearchParams } from "next/navigation";

import { useConsoleShell } from "@/components/console-shell";
import { CopyButton } from "@/components/copy-button";
import { SidePanel, SidePanelHeader } from "@/components/side-panel";
import { PanelEdge, usePanelWidth } from "@/components/traces/panel-width";
import {
  FullSizeAction,
  OpenPageAction,
  PeekNote,
  ReadFailure,
  StepActions,
  neighbours,
  readTrace,
  usePeekEntry,
  usePeekRead,
} from "@/components/traces/peek-read";
import { TraceScores, TraceScoresProvider } from "@/components/traces/trace-scores";
import { TraceBody, TraceMeta } from "@/components/traces/trace-view";
import { MediaProjectProvider } from "@/components/traces/span-content";
import { openPeek, peekedTrace } from "@/lib/peek";
import { summarizeTrace } from "@/lib/trace-tree";
import { commentsOf } from "@/lib/scores";
import { PEEK_PARAM, shortId, traceHref, traceReadHref } from "@/lib/traces";
import { offerPageSteps } from "@/lib/use-page-steps";
import { cn } from "@/lib/utils";
import { ContentMode } from "@/lib/types/enums";

// A trace opened from the list without leaving it: the console's side panel
// (`side-panel.tsx`) over the list, which fills
// the content area at a press and goes back. The trace it shows is the URL's
// `?peek=`, kept in the browser's history (`lib/peek.ts`), so a link holds it
// and Back shuts it. The list's filters, sorts and pages carry no `peek`, so
// they shut it as they move the list; a new window keeps it, the window
// picker carrying the whole query along.

/** The trace the peek holds open, from the URL; `null` while it is shut. */
export function usePeekedTrace(): string | null {
  return peekedTrace(useSearchParams().get(PEEK_PARAM));
}

type Props = {
  organization: string;
  slug: string;
  /** The project's id, which its reads are named by. */
  projectId: string;
  /** The page's traces in the list's order: what the ↑ and ↓ buttons, and
   *  `j` and `k`, step through. */
  traces: readonly string[];
  /** The project's content mode, said on the trace's line. */
  contentMode?: ContentMode;
  /** The person's own user id, and whether they may take back any member's
   *  score, as the trace's page has them. */
  me: string;
  administers: boolean;
};

export function TracePeek(props: Props) {
  const traceId = usePeekedTrace();
  const { element } = useConsoleShell();
  if (!traceId || !element) return null;
  return createPortal(<PeekPanel {...props} traceId={traceId} />, element);
}

function PeekPanel({ organization, slug, projectId, traceId, traces, contentMode, me, administers }: Props & { traceId: string }) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { full, width, toggleFull, edge } = usePanelWidth(panel);
  // A trace still going is read again as its spans land.
  const { read, stale, again } = usePeekRead(traceReadHref(traceId, projectId), readTrace);
  const { previous, next } = neighbours(traces, traceId);
  const trace = read?.kind === "ok" ? read.value : null;
  // Summed once a trace, not at each draw: a drag of the panel's edge draws
  // it at every move of the pointer.
  const summary = useMemo(() => (trace ? summarizeTrace(trace.spans) : null), [trace]);
  // The trace on show: the one read while the next is on its way.
  const shown = trace?.trace_id ?? traceId;

  const close = usePeekEntry();

  // The trace open as the panel shuts, for the focus to go back to its row,
  // as the agent's window hands it back (`agent-panel.tsx`); the panel calls
  // the latest of these as it shuts.
  const openRow = () =>
    [...document.querySelectorAll<HTMLElement>("[data-peek-trace]")].find(
      (link) => link.dataset.peekTrace === traceId,
    );

  // `j` and `k` step as the ↓ and ↑ buttons do, while the panel is open. Each
  // press reads where the peek stands then, from the URL, so two in quick
  // succession step twice rather than both from where the panel was drawn.
  const step = useEffectEvent((by: number) => {
    const now = new URLSearchParams(window.location.search).get(PEEK_PARAM) ?? "";
    const to = traces[traces.indexOf(now) + by];
    if (traces.includes(now) && to) openPeek(to);
  });
  useEffect(
    () => offerPageSteps({ noun: "trace", next: () => step(1), previous: () => step(-1) }),
    [],
  );

  // Another trace on show takes the place of the last, and with it whatever
  // in it had the focus: the panel takes it back, so Escape, `j` and `k`
  // still reach it. A row of the list clicked to switch it hands it over too,
  // as the first click did. The ↑ and ↓ buttons keep theirs.
  useLayoutEffect(() => {
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus({ preventScroll: true });
  }, [trace?.trace_id]);

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
            <CopyButton value={shown} label="the trace's id" />
          </>
        }
        actions={
          <>
            <OpenPageAction href={traceHref(organization, slug, traceId)} label="Open the trace's page" />
            <StepActions
              noun="trace"
              previous={previous}
              next={next}
            />
            <FullSizeAction full={full} toggleFull={toggleFull} />
          </>
        }
        onClose={close}
      >
        {trace && (
          <TraceMeta
            trace={trace}
            organization={organization}
            slug={slug}
            incidents={trace.incidents}
            contentMode={contentMode}
          />
        )}
        {/* Its scores, and a member's own to give, as on the trace's page. */}
        {trace?.scores && (
          <div aria-label="Scores" role="group" className="flex flex-wrap items-center gap-1">
            <TraceScores
              projectId={projectId}
              traceId={trace.trace_id}
              scores={trace.scores}
              comments={commentsOf(trace.content)}
              spanNames={Object.fromEntries(trace.spans.map((span) => [span.span_id, span.name]))}
              me={me}
              administers={administers}
              keepsContent={contentMode === ContentMode.On}
              onChanged={again}
            />
          </div>
        )}
      </SidePanelHeader>
      <div
        aria-busy={read === null || stale}
        className={cn("flex min-h-0 flex-1 flex-col", stale && "opacity-60 transition-opacity")}
      >
        {read === null ? (
          <PeekNote>Reading the trace&hellip;</PeekNote>
        ) : read.kind === "ok" ? (
          <MediaProjectProvider projectId={projectId}>
            <TraceScoresProvider scores={read.value.scores ?? []} comments={commentsOf(read.value.content)}>
              <TraceBody
                key={read.value.trace_id}
                trace={read.value}
                organization={organization}
                slug={slug}
                className="min-h-0 flex-1"
              />
            </TraceScoresProvider>
          </MediaProjectProvider>
        ) : (
          <ReadFailure
            read={read}
            noun="trace"
            missing={{
              title: "No trace with this id in this project.",
              body: "It may have been deleted, its spans may have passed their retention, or the trace belongs to another project.",
            }}
            retry={again}
          />
        )}
      </div>
    </SidePanel>
  );
}
