"use client";

import { useEffect, useId, useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { MessagesSquare } from "lucide-react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";

import { useConsoleShell } from "@/components/console-shell";
import { CopyButton } from "@/components/copy-button";
import { LocalTime } from "@/components/local-time";
import { SidePanel, SidePanelHeader } from "@/components/side-panel";
import { LayoutControls } from "@/components/traces/layout-controls";
import { PanelEdge, usePanelWidth } from "@/components/traces/panel-width";
import {
  FullSizeAction,
  OpenPageAction,
  PeekNote,
  ReadFailure,
  StepActions,
  neighbours,
  readSession,
  usePeekEntry,
  usePeekRead,
} from "@/components/traces/peek-read";
import { SessionRuns } from "@/components/traces/session-runs";
import { LayoutProvider, useLayout, type LayoutPage } from "@/components/traces/use-layout";
import { MAX_LIMIT, type WindowQuery } from "@/lib/list-query";
import { landOnList, peekUrl } from "@/lib/peek";
import { sessionHref, sessionReadHref, sessionSums } from "@/lib/sessions";
import { PEEK_PARAM, formatCount, tracesHref } from "@/lib/traces";
import { cn } from "@/lib/utils";

// A session opened from the list without leaving it, in the console's side
// panel (`side-panel.tsx`), as a trace is opened from the trace list
// (`peek.tsx`) and as New project opens: the session's sums on the header's
// line, and under them its runs under the page's layout, with Customize and
// Layouts as the session's page has them. The session it shows is the URL's
// `?peek=`, kept in the browser's history (`lib/peek.ts`), so a link holds
// it and Back shuts it; the ↑ and ↓ buttons step the list's sessions, while
// `j`, `k` and `o` are the runs' inside, as on the session's page.

/** The longest a session's id is, in characters as the server counts them. */
const SESSION_ID_MAX = 200;

/** The session a `?peek=` value names, or `null` for a value that names
 *  none: a session id is the application's own text. */
export function peekedSession(value: string | null): string | null {
  const id = value?.trim() ?? "";
  return id && [...id].length <= SESSION_ID_MAX ? id : null;
}

/** The session the panel holds open, from the URL; `null` while it is shut. */
export function usePeekedSession(): string | null {
  return peekedSession(useSearchParams().get(PEEK_PARAM));
}

type Props = LayoutPage & {
  organization: string;
  slug: string;
  /** The project's id, which its reads are named by. */
  projectId: string;
  /** The page's sessions in the list's order: what the ↑ and ↓ buttons step
   *  through. */
  sessions: readonly string[];
  /** The list's window, which the session is read in. */
  window: WindowQuery;
  /** Whether the person may save layouts and set the project's default. */
  canSave: boolean;
};

export function SessionPeek(props: Props) {
  const sessionId = usePeekedSession();
  const { element } = useConsoleShell();
  if (!sessionId || !element) return null;
  return createPortal(
    <LayoutProvider layouts={props.layouts} project={props.project}>
      <SessionPanel {...props} sessionId={sessionId} />
    </LayoutProvider>,
    element,
  );
}

function SessionPanel({
  organization,
  slug,
  projectId,
  sessions,
  window: query,
  canSave,
  sessionId,
}: Props & { sessionId: string }) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { full, width, toggleFull, edge } = usePanelWidth(panel);
  // Read with the layout's pins, which the rows take as theirs: a pin changed reads once.
  const { options } = useLayout();
  // Read again as the project's runs land, the list beside it redrawn.
  const { read, stale, again } = usePeekRead(
    sessionReadHref(sessionId, projectId, query, options.pinned),
    readSession,
  );
  const { previous, next } = neighbours(sessions, sessionId);
  const session = read?.kind === "ok" ? read.value : null;
  // The session on show: the one read while the next is on its way.
  const shown = session?.session_id ?? sessionId;

  const close = usePeekEntry();

  // The browser's own Back lands on the list as Close does (`landOnList`), from
  // the list's address as the peek last stood: once, off the peek's own entries.
  const shut = useRef("");
  useEffect(() => {
    shut.current = peekUrl(null);
  });
  useEffect(() => {
    const landed = () => {
      // Onto another of the peek's own entries: the landing is kept for the list.
      if (new URLSearchParams(window.location.search).has(PEEK_PARAM)) return;
      const from = shut.current;
      shut.current = "";
      if (from) landOnList(from);
    };
    window.addEventListener("popstate", landed);
    // Let go after the event in hand: Next commits the list inside the popstate
    // itself, which unmounts the panel before this listener's turn comes.
    return () => void setTimeout(() => window.removeEventListener("popstate", landed));
  }, []);

  // The session open as the panel shuts, for the focus to go back to its row;
  // the panel calls the latest of these as it shuts.
  const openRow = () =>
    [...document.querySelectorAll<HTMLElement>("[data-peek-session]")].find(
      (link) => link.dataset.peekSession === sessionId,
    );

  // Another session on show takes the place of the last, and the panel takes
  // the focus back with it, so Escape still reaches it.
  useLayoutEffect(() => {
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus({ preventScroll: true });
  }, [session?.session_id]);

  const page = { organization, slug, projectId };
  const sums = session ? sessionSums(session.traces) : null;

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
        icon={MessagesSquare}
        titleId={titleId}
        title={
          <span className="text-sm" title={shown}>
            {shown}
          </span>
        }
        beside={<CopyButton value={shown} label="the session's id" />}
        actions={
          <>
            <OpenPageAction
              href={sessionHref(organization, slug, sessionId, query)}
              label="Open the session's page"
            />
            <StepActions noun="session" previous={previous} next={next} />
            <FullSizeAction full={full} toggleFull={toggleFull} />
          </>
        }
        onClose={close}
      >
        {session && sums && (
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
            {sums.user ? (
              <Link
                href={tracesHref(organization, slug, { user: [sums.user], days: query.days })}
                className="text-xs underline-offset-4 hover:underline"
              >
                user {sums.user}
              </Link>
            ) : (
              <span>no user</span>
            )}
            <span className="tabular-nums">
              {formatCount(session.traces.length)}
              {session.more ? " traces shown" : session.traces.length === 1 ? " trace" : " traces"}
              {sums.errors > 0 && (
                <span className="text-destructive">, {formatCount(sums.errors)} with failed spans</span>
              )}
            </span>
            <span className="tabular-nums">{formatCount(sums.steps)} steps</span>
            <span
              className="tabular-nums"
              title={`${formatCount(sums.inputTokens)} in, ${formatCount(sums.outputTokens)} out`}
            >
              {formatCount(sums.inputTokens + sums.outputTokens)} tokens
            </span>
            <span>
              <LocalTime iso={session.traces[0]!.started_at} /> to{" "}
              <LocalTime iso={sums.lastEnded} />
            </span>
          </p>
        )}
      </SidePanelHeader>
      <div
        aria-busy={read === null || stale}
        className={cn(
          "flex min-h-0 flex-1 flex-col overflow-y-auto",
          stale && "opacity-60 transition-opacity",
        )}
      >
        {read === null ? (
          <PeekNote>Reading the session&hellip;</PeekNote>
        ) : read.kind === "ok" ? (
          <div className="grid content-start gap-3 p-gutter">
            <div>
              <LayoutControls projectId={projectId} canSave={canSave} />
            </div>
            <SessionRuns
              key={read.value.session_id}
              {...page}
              sessionId={read.value.session_id}
              traces={read.value.traces}
              readWith={options.pinned}
              window={query}
            />
            {read.value.more && (
              <p className="text-sm text-muted-foreground">
                The first {formatCount(read.value.limit)} of more in this window.{" "}
                <Link
                  href={sessionHref(organization, slug, sessionId, { ...query, limit: MAX_LIMIT })}
                  className="underline underline-offset-4"
                >
                  Open the session&rsquo;s page for up to {formatCount(MAX_LIMIT)}
                </Link>
                , or narrow the window.
              </p>
            )}
          </div>
        ) : (
          <ReadFailure
            read={read}
            noun="session"
            missing={{
              title: "No session with this id in this project's window.",
              body: "Its runs may lie outside the window or have passed their retention, or the session belongs to another project.",
            }}
            retry={again}
          />
        )}
      </div>
    </SidePanel>
  );
}
