"use client";

import { useEffect, useId, useLayoutEffect, useMemo, useRef } from "react";
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
import { TraceBody, TraceMeta } from "@/components/traces/trace-view";
import { NEXT_KEY, PREVIOUS_KEY } from "@/lib/keys";
import { openPeek, peekedRun } from "@/lib/peek";
import { summarizeTrace } from "@/lib/trace-tree";
import { PEEK_PARAM, shortId, traceHref, traceReadHref } from "@/lib/traces";
import { offerPageSteps } from "@/lib/use-page-steps";
import { cn } from "@/lib/utils";

// A trace opened from the list without leaving it: the console's side panel
// (`side-panel.tsx`) over the list, which fills
// the content area at a press and goes back. The run it shows is the URL's
// `?peek=`, kept in the browser's history (`lib/peek.ts`), so a link holds it
// and Back shuts it. The list's filters, sorts and pages carry no `peek`, so
// they shut it as they move the list; a new window keeps it, the window
// picker carrying the whole query along.

/** The run the peek holds open, from the URL; `null` while it is shut. */
export function usePeekedRun(): string | null {
  return peekedRun(useSearchParams().get(PEEK_PARAM));
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
  // A run still going is read again as its spans land.
  const { read, stale, again } = usePeekRead(traceReadHref(run, projectId), readTrace);
  const { previous, next } = neighbours(runs, run);
  const trace = read?.kind === "ok" ? read.trace : null;
  // Summed once a trace, not at each draw: a drag of the panel's edge draws
  // it at every move of the pointer.
  const summary = useMemo(() => (trace ? summarizeTrace(trace.spans) : null), [trace]);
  // The run on show: the one read while the next is on its way.
  const shown = trace?.run_id ?? run;

  const close = usePeekEntry();

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
            <OpenPageAction href={traceHref(organization, slug, run)} label="Open the trace's page" />
            <StepActions
              noun="trace"
              previous={previous}
              next={next}
              keys={{ previous: PREVIOUS_KEY, next: NEXT_KEY }}
            />
            <FullSizeAction full={full} toggleFull={toggleFull} />
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
          <PeekNote>Reading the trace&hellip;</PeekNote>
        ) : read.kind === "ok" ? (
          <TraceBody
            key={read.trace.run_id}
            trace={read.trace}
            organization={organization}
            slug={slug}
            className="min-h-0 flex-1"
          />
        ) : (
          <ReadFailure
            read={read}
            noun="trace"
            missing={{
              title: "No trace with this id in this project.",
              body: "Its spans may have passed their retention, or the run belongs to another project.",
            }}
            retry={again}
          />
        )}
      </div>
    </SidePanel>
  );
}
