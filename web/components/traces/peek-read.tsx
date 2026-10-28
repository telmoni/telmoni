"use client";

import { ArrowDown, ArrowUp, Maximize2, Minimize2, SquareArrowOutUpRight } from "lucide-react";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { PanelAction } from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  closePeek,
  followEntry,
  keepRead,
  openPeek,
  pushedEntry,
  shownRead,
  type KeptRead,
  type TraceRead,
} from "@/lib/peek";
import type { SessionDetail } from "@/lib/server/data";
import type { TraceDetail } from "@/lib/server/entities/traces";
import { PEEK_NUDGED } from "@/lib/traces";

// What the trace peek, the session peek and a session's opened trace share:
// the read of a lane, its entry in the history, the rows beside the one open,
// the header's actions, and what a panel says when a read did not land.

/** How long a read may take before the panel says it failed. */
const READ_TIMEOUT_MS = 15_000;

/** The lane's answer as a read, its status the read's kind; a body `holds`
 *  rejects is missing. */
async function readJsonAs<T>(
  url: string,
  signal: AbortSignal,
  holds: (body: T) => boolean,
): Promise<TraceRead<T>> {
  try {
    const res = await fetch(url, { signal, cache: "no-store", headers: { accept: "application/json" } });
    if (res.status === 401) return { kind: "signed-out" };
    if (res.status === 404) return { kind: "missing" };
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const body = (await res.json()) as T;
    return holds(body) ? { kind: "ok", trace: body } : { kind: "missing" };
  } catch {
    return { kind: "unavailable" };
  }
}

/** A run's spans, through the trace list's own route: none is a run missing. */
export const readTrace = (url: string, signal: AbortSignal) =>
  readJsonAs<TraceDetail>(url, signal, (trace) => trace.spans.length > 0);

/** A session's traces in a window, through its own route: none is a session
 *  missing. `TraceRead` names what it holds a trace; here it is the session. */
export const readSession = (url: string, signal: AbortSignal) =>
  readJsonAs<SessionDetail>(
    url,
    signal,
    (session) => Array.isArray(session.traces) && session.traces.length > 0,
  );

/** `read` of `url` started, and the cleanup that ends it, after which `settle`
 *  is not called. One controller, aborted by its timer or the cleanup: joining
 *  two signals (`AbortSignal.any`) is newer than browsers Next supports. */
export function startRead<T>(
  url: string,
  read: (url: string, signal: AbortSignal) => Promise<TraceRead<T>>,
  settle: (read: TraceRead<T>) => void,
): () => void {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), READ_TIMEOUT_MS);
  let current = true;
  void read(url, controller.signal).then((answer) => {
    clearTimeout(timer);
    if (current) settle(answer);
  });
  return () => {
    current = false;
    clearTimeout(timer);
    controller.abort();
  };
}

/** `url` read when it changes, at each nudge (`PEEK_NUDGED`: the project's
 *  runs landing) and at `again`, a retry; kept and shown as `lib/peek.ts`
 *  keeps a read. */
export function usePeekRead<T>(
  url: string,
  read: (url: string, signal: AbortSignal) => Promise<TraceRead<T>>,
) {
  const [version, setVersion] = useState(0);
  const [last, setLast] = useState<KeptRead<T> | null>(null);
  useEffect(
    () =>
      startRead(url, read, (answer) =>
        setLast((was) => keepRead(was, { url, version, read: answer })),
      ),
    [url, version, read],
  );
  const again = useCallback(() => setVersion((n) => n + 1), []);
  useEffect(() => {
    window.addEventListener(PEEK_NUDGED, again);
    return () => window.removeEventListener(PEEK_NUDGED, again);
  }, [again]);
  return { ...shownRead(last, url, version), again };
}

/** The entry the panel pushed, followed through each draw (a refresh's, an
 *  action's, the list's move to another window), so shutting goes back over
 *  it only while it is the one in front. Answers the shut. */
export function usePeekEntry(): () => void {
  const entry = useRef<string | null>(null);
  useLayoutEffect(() => {
    entry.current = pushedEntry();
  }, []);
  useEffect(() => {
    entry.current = followEntry(entry.current);
  });
  return () => closePeek(entry.current !== null);
}

/** The rows beside `current` among `items`: what the ↑ and ↓ buttons step to. */
export function neighbours(
  items: readonly string[],
  current: string,
): { previous: string | null; next: string | null } {
  const at = items.indexOf(current);
  return {
    previous: at > 0 ? items[at - 1]! : null,
    next: at !== -1 && at + 1 < items.length ? items[at + 1]! : null,
  };
}

/** What a panel says in its row's place. */
export function PeekNote({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <div className="grid gap-1 p-3.5 text-sm">
      {title && <p className="font-medium">{title}</p>}
      <p className="text-muted-foreground">{children}</p>
    </div>
  );
}

/** The way back in once the session has ended, to the page as it stands. */
export function SignInAgain() {
  const pathname = usePathname();
  const search = useSearchParams().toString();
  return (
    <a
      href={`/auth/login?returnTo=${encodeURIComponent(`${pathname}?${search}`)}`}
      className="underline underline-offset-4"
    >
      Sign in again
    </a>
  );
}

/** What a panel says when a read did not land: the `noun` missing, in the
 *  caller's words; not the reader's; the session ended; or a read to try again. */
export function ReadFailure({
  read,
  noun,
  missing,
  retry,
}: {
  read: Exclude<TraceRead<unknown>, { kind: "ok" }>;
  noun: "trace" | "session";
  missing: { title: string; body: ReactNode };
  retry: () => void;
}) {
  if (read.kind === "missing") return <PeekNote title={missing.title}>{missing.body}</PeekNote>;
  if (read.kind === "forbidden") {
    return (
      <PeekNote title={`This project's ${noun}s are not yours to read.`}>
        Ask an owner of the project for a role that reads them.
      </PeekNote>
    );
  }
  if (read.kind === "signed-out") {
    return (
      <PeekNote title="Your session has ended.">
        <SignInAgain /> to read it.
      </PeekNote>
    );
  }
  return (
    <PeekNote title={`The ${noun} could not be read.`}>
      <button type="button" onClick={retry} className="cursor-pointer underline underline-offset-4">
        Try again
      </button>{" "}
      in a moment.
    </PeekNote>
  );
}

/** The header's way to the row's own page. */
export function OpenPageAction({ href, label }: { href: string; label: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon" className="text-muted-foreground hover:text-foreground" asChild>
          <Link href={href} aria-label={label}>
            <SquareArrowOutUpRight />
          </Link>
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** The header's ↑ and ↓, stepping the page's rows; `keys` names their letter
 *  keys where the panel offers them. */
export function StepActions({
  noun,
  previous,
  next,
  keys,
}: {
  noun: string;
  previous: string | null;
  next: string | null;
  keys?: { previous: string; next: string };
}) {
  return (
    <>
      <PanelAction
        label={`Previous ${noun}`}
        {...(keys ? { letterKey: keys.previous } : {})}
        unavailable={!previous}
        onClick={() => previous && openPeek(previous)}
      >
        <ArrowUp />
      </PanelAction>
      <PanelAction
        label={`Next ${noun}`}
        {...(keys ? { letterKey: keys.next } : {})}
        unavailable={!next}
        onClick={() => next && openPeek(next)}
      >
        <ArrowDown />
      </PanelAction>
    </>
  );
}

/** The header's full-size toggle, named as the agent's window names its own;
 *  a phone's width has the panel at full size already. */
export function FullSizeAction({ full, toggleFull }: { full: boolean; toggleFull: () => void }) {
  return (
    <PanelAction
      label={full ? "Exit full size" : "Full size"}
      onClick={toggleFull}
      className="max-md:hidden"
    >
      {full ? <Minimize2 /> : <Maximize2 />}
    </PanelAction>
  );
}
