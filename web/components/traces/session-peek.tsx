"use client";

import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import {
  ArrowDown,
  ArrowUp,
  Maximize2,
  MessagesSquare,
  Minimize2,
  SquareArrowOutUpRight,
} from "lucide-react";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";

import { useConsoleShell } from "@/components/console-shell";
import { CopyButton } from "@/components/copy-button";
import { LocalTime } from "@/components/local-time";
import { PanelAction, SidePanel, SidePanelHeader } from "@/components/side-panel";
import { LayoutControls } from "@/components/traces/layout-controls";
import { PanelEdge, usePanelWidth } from "@/components/traces/panel-width";
import { SessionRuns } from "@/components/traces/session-runs";
import { useLayout, type LayoutPage } from "@/components/traces/use-layout";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MAX_LIMIT, type WindowQuery } from "@/lib/list-query";
import {
  closePeek,
  followEntry,
  keepRead,
  landOnList,
  openPeek,
  peekUrl,
  pushedEntry,
  shownRead,
  type KeptRead,
  type TraceRead,
} from "@/lib/peek";
import type { SessionDetail } from "@/lib/server/data";
import { sessionHref, sessionSums } from "@/lib/sessions";
import { PEEK_NUDGED, PEEK_PARAM, formatCount, tracesHref } from "@/lib/traces";
import { cn } from "@/lib/utils";

// A session opened from the list without leaving it, in the console's side
// panel (`side-panel.tsx`), as a trace is opened from the trace list
// (`peek.tsx`) and as New project opens: the session's sums on the header's
// line, and under them its runs under the page's layout, with Customize and
// Layouts as the session's page has them. The session it shows is the URL's
// `?peek=`, kept in the browser's history (`lib/peek.ts`), so a link holds
// it and Back shuts it; the ↑ and ↓ buttons step the list's sessions, while
// `j`, `k` and `o` are the runs' inside, as on the session's page.

/** How long a read of the session may take before the panel says it failed. */
const READ_TIMEOUT_MS = 15_000;

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

async function readSession(url: string, signal: AbortSignal): Promise<TraceRead<SessionDetail>> {
  try {
    const res = await fetch(url, { signal, cache: "no-store", headers: { accept: "application/json" } });
    if (res.status === 401) return { kind: "signed-out" };
    if (res.status === 404) return { kind: "missing" };
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const session = (await res.json()) as SessionDetail;
    return Array.isArray(session.traces) && session.traces.length > 0
      ? { kind: "ok", trace: session }
      : { kind: "missing" };
  } catch {
    return { kind: "unavailable" };
  }
}

/** The session's traces, read when it is opened and again at each
 *  `version`, a retry; kept and shown as a trace's are (`lib/peek.ts`),
 *  which names what a read holds a trace: here it is the session. */
function useSessionRead(url: string, version: number) {
  const [last, setLast] = useState<KeptRead<SessionDetail> | null>(null);
  useEffect(() => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), READ_TIMEOUT_MS);
    let current = true;
    void readSession(url, controller.signal).then((read) => {
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
  return createPortal(<SessionPanel {...props} sessionId={sessionId} />, element);
}

function SessionPanel({
  organization,
  slug,
  projectId,
  sessions,
  window: query,
  layouts,
  project,
  canSave,
  sessionId,
}: Props & { sessionId: string }) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { full, width, toggleFull, edge } = usePanelWidth(panel);
  // Read with the layout's pins, which the rows take as theirs: a pin changed reads once.
  const { options } = useLayout({ layouts, project });
  const params = new URLSearchParams({ project: projectId });
  if (query.days !== undefined) params.set("days", String(query.days));
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  for (const name of options.pinned) params.append("pinned", name);
  const url = `/api/v1/sessions/${encodeURIComponent(sessionId)}?${params}`;
  const [version, setVersion] = useState(0);
  const { read, stale } = useSessionRead(url, version);
  // Read again as the project's runs land, the list beside it redrawn.
  useEffect(() => {
    const again = () => setVersion((n) => n + 1);
    window.addEventListener(PEEK_NUDGED, again);
    return () => window.removeEventListener(PEEK_NUDGED, again);
  }, []);
  const pathname = usePathname();
  const search = useSearchParams().toString();
  const at = sessions.indexOf(sessionId);
  const previous = at > 0 ? sessions[at - 1]! : null;
  const next = at !== -1 && at + 1 < sessions.length ? sessions[at + 1]! : null;
  const session = read?.kind === "ok" ? read.trace : null;
  // The session on show: the one read while the next is on its way.
  const shown = session?.session_id ?? sessionId;

  // The entry the panel pushed, followed through each draw, so shutting goes
  // back over it only while it is the one in front (`peek.tsx`).
  const entry = useRef<string | null>(null);
  useLayoutEffect(() => {
    entry.current = pushedEntry();
  }, []);
  useEffect(() => {
    entry.current = followEntry(entry.current);
  });
  const close = () => closePeek(entry.current !== null);

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

  // The session open as the panel shuts, for the focus to go back to its row.
  const openId = useRef(sessionId);
  useEffect(() => {
    openId.current = sessionId;
  }, [sessionId]);
  const openRow = () =>
    [...document.querySelectorAll<HTMLElement>("[data-peek-session]")].find(
      (link) => link.dataset.peekSession === openId.current,
    );

  // Another session on show takes the place of the last, and the panel takes
  // the focus back with it, so Escape still reaches it.
  useLayoutEffect(() => {
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus({ preventScroll: true });
  }, [session?.session_id]);

  const page = { organization, slug, projectId, layouts, project };
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
          <span className="font-mono text-sm" title={shown}>
            {shown}
          </span>
        }
        beside={<CopyButton value={shown} label="the session's id" />}
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
                  <Link
                    href={sessionHref(organization, slug, sessionId, query)}
                    aria-label="Open the session's page"
                  >
                    <SquareArrowOutUpRight />
                  </Link>
                </Button>
              </TooltipTrigger>
              <TooltipContent>Open the session&rsquo;s page</TooltipContent>
            </Tooltip>
            <PanelAction
              label="Previous session"
              unavailable={!previous}
              onClick={() => previous && openPeek(previous)}
            >
              <ArrowUp />
            </PanelAction>
            <PanelAction
              label="Next session"
              unavailable={!next}
              onClick={() => next && openPeek(next)}
            >
              <ArrowDown />
            </PanelAction>
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
        {session && sums && (
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
            {sums.user ? (
              <Link
                href={tracesHref(organization, slug, { user: [sums.user], days: query.days })}
                className="font-mono text-xs underline-offset-4 hover:underline"
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
          <Note>Reading the session&hellip;</Note>
        ) : read.kind === "ok" ? (
          <div className="grid content-start gap-3 p-3.5">
            <div>
              <LayoutControls {...page} canSave={canSave} />
            </div>
            <SessionRuns
              key={read.trace.session_id}
              {...page}
              sessionId={read.trace.session_id}
              traces={read.trace.traces}
              readWith={options.pinned}
              window={query}
            />
            {read.trace.more && (
              <p className="text-sm text-muted-foreground">
                The first {formatCount(read.trace.limit)} of more in this window.{" "}
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
        ) : read.kind === "missing" ? (
          <Note title="No session with this id in this project's window.">
            Its runs may lie outside the window or have passed their retention, or the session
            belongs to another project.
          </Note>
        ) : read.kind === "forbidden" ? (
          <Note title="This project's sessions are not yours to read.">
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
          <Note title="The session could not be read.">
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

/** What the panel says in the session's place. */
function Note({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 p-3.5 text-sm">
      {title && <p className="font-medium">{title}</p>}
      <p className="text-muted-foreground">{children}</p>
    </div>
  );
}
