"use client";

import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import {
  ArrowDown,
  ArrowUp,
  ListTree,
  Maximize2,
  Minimize2,
  SquareArrowOutUpRight,
  XIcon,
} from "lucide-react";
import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";

import { useConsoleShell } from "@/components/console-shell";
import { CopyButton } from "@/components/copy-button";
import { TraceBody, TraceMeta } from "@/components/traces/trace-view";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { NEXT_KEY, PREVIOUS_KEY } from "@/lib/keys";
import {
  closePeek,
  draggedWidth,
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
import { PEEK_PARAM, PEEKED_RUN_LANDED, shortId, traceHref } from "@/lib/traces";
import { offerPageSteps } from "@/lib/use-page-steps";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { cn } from "@/lib/utils";

// A trace opened from the list without leaving it, as Langfuse's peek opens
// one: a panel over the list's right side, inside the content area — under
// the header, beside the rail, neither covered — that fills that area at a
// press and goes back. The run it shows is the URL's `?peek=`, kept in the
// browser's history (`lib/peek.ts`), so a link holds it and Back shuts it.
// The list's filters, sorts and pages carry no `peek`, so they shut it as
// they move the list; a new window keeps it, the window picker carrying the
// whole query along.

/** How long a read of the spans may take before the panel says it failed. */
const READ_TIMEOUT_MS = 15_000;

/** The run the peek holds open, from the URL; `null` while it is shut. */
export function usePeekedRun(): string | null {
  return peekedRun(useSearchParams().get(PEEK_PARAM));
}

/** Whether the panel stands as wide as the content area it is in: on a
 *  phone, beside a list too narrow for its own width, or dragged all the
 *  way — measured, since each of those is the window's or the hand's. */
function useCoversArea(panel: RefObject<HTMLDivElement | null>): boolean {
  const [covers, setCovers] = useState(false);
  useLayoutEffect(() => {
    const element = panel.current;
    const area = element?.parentElement;
    if (!element || !area || typeof ResizeObserver === "undefined") return;
    // An area not yet laid out measures nothing, and covers nothing either.
    const measure = () => {
      const whole = area.getBoundingClientRect().width;
      setCovers(whole > 0 && element.getBoundingClientRect().width >= whole - 1);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    observer.observe(area);
    measure();
    return () => observer.disconnect();
  }, [panel]);
  return covers;
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

/** One of the panel's buttons: an icon, named for a screen reader and, under
 *  the pointer, in a tooltip with its letter key where it has one. At the
 *  end of the list a step is `unavailable` rather than disabled, so the focus
 *  stays on it. */
function PanelAction({
  label,
  letterKey,
  unavailable,
  onClick,
  children,
  className,
  ...props
}: {
  label: string;
  letterKey?: string;
  unavailable?: boolean;
  children: ReactNode;
} & Omit<ComponentProps<typeof Button>, "aria-label" | "aria-disabled">) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={label}
          aria-disabled={unavailable || undefined}
          onClick={unavailable ? undefined : onClick}
          className={cn("aria-disabled:cursor-not-allowed aria-disabled:opacity-50", className)}
          {...props}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        {label}
        {letterKey && (
          <Kbd data-letter-key="" className="ml-1.5">
            {letterKey.toUpperCase()}
          </Kbd>
        )}
      </TooltipContent>
    </Tooltip>
  );
}

function PeekPanel({ organization, slug, projectId, run, runs }: Props & { run: string }) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const { setCovered } = useConsoleShell();
  const { peekFull, peekWidth } = useTracePrefs();
  const coversArea = useCoversArea(panel);
  const [version, setVersion] = useState(0);
  const { read, stale } = useTraceRead(
    `/api/traces/${encodeURIComponent(run)}?project=${encodeURIComponent(projectId)}`,
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

  // Over the list whole — at full size, or as wide as the area otherwise —
  // the list is out of reach of the keyboard and of a screen reader too, as
  // it is of the eye.
  const covering = peekFull || coversArea;
  useLayoutEffect(() => {
    setCovered(covering);
    return () => setCovered(false);
  }, [covering, setCovered]);

  // The run open as the panel shuts, for the focus to go back to its row.
  const openRun = useRef(run);
  useEffect(() => {
    openRun.current = run;
  }, [run]);

  // A run still going is read again as its spans land.
  useEffect(() => {
    const again = () => setVersion((n) => n + 1);
    window.addEventListener(PEEKED_RUN_LANDED, again);
    return () => window.removeEventListener(PEEKED_RUN_LANDED, again);
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

  // Focus on open, and back to the row of the run it was showing as it shuts
  // — as the agent's window does (`agent-panel.tsx`) — but only from inside
  // it: shut by a click on the list, a filter or a page, the focus is that
  // click's. A layout effect, so the panel is still in the document when its
  // cleanup asks where the focus is.
  useLayoutEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const element = panel.current;
    element?.focus({ preventScroll: true });
    return () => {
      if (!element?.contains(document.activeElement)) return;
      // The list may still be covered as the panel shuts at full size, and
      // nothing covered takes the focus.
      setCovered(false);
      const row = [...document.querySelectorAll<HTMLElement>("[data-peek-run]")].find(
        (link) => link.dataset.peekRun === openRun.current,
      );
      (row ?? before)?.focus({ preventScroll: true });
    };
  }, [setCovered]);

  // Another trace on show takes the place of the last, and with it whatever
  // in it had the focus: the panel takes it back, so Escape, `j` and `k`
  // still reach it. A row of the list clicked to switch it hands it over too,
  // as the first click did. The ↑ and ↓ buttons keep theirs.
  useLayoutEffect(() => {
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus({ preventScroll: true });
  }, [trace?.run_id]);

  // Its left edge dragged, as the agent's window is sized by its own
  // (`agent-window.tsx`): followed as the pointer moves, and kept with the
  // list's other choices once it lets go — one write a drag.
  const [dragged, setDragged] = useState<number | null>(null);
  const drag = useRef<{ pointerId: number; x: number; start: number; width: number | null } | null>(
    null,
  );
  const beginDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !e.isPrimary || !panel.current) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const start = panel.current.getBoundingClientRect().width;
    drag.current = { pointerId: e.pointerId, x: e.clientX, start, width: null };
  };
  const trackDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    const area = panel.current?.parentElement?.getBoundingClientRect().width;
    if (!d || d.pointerId !== e.pointerId || !area) return;
    d.width = draggedWidth(d.start, d.x, e.clientX, area);
    setDragged(d.width);
  };
  const endDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointerId !== e.pointerId) return;
    drag.current = null;
    if (d.width !== null) setTracePrefs({ peekWidth: d.width });
    setDragged(null);
  };
  const width = dragged ?? peekWidth;

  // Escape from inside shuts it. Only from inside: the panel does not shut
  // the page off, so Escape on the list is the list's.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (!panel.current?.contains(document.activeElement)) return;
      e.preventDefault();
      closePeek(entry.current !== null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div
      ref={panel}
      role="dialog"
      // Non-modal, and said so: the list beside it keeps its pointer and
      // its keys, which the letter shortcuts read this to know
      // (`MODAL_SELECTOR`, `lib/keys.ts`).
      aria-modal="false"
      aria-labelledby={titleId}
      tabIndex={-1}
      data-mode={peekFull ? "full" : "side"}
      style={width === null ? undefined : ({ "--peek-width": `${width}px` } as CSSProperties)}
      className={cn(
        "absolute inset-y-0 right-0 z-30 flex flex-col overflow-hidden bg-background outline-none",
        "animate-in duration-200 fade-in-0 slide-in-from-right-4",
        peekFull
          ? "left-0"
          : cn(
              "left-0 md:left-auto md:max-w-full md:border-l md:border-border md:shadow-drawer",
              width === null ? "md:w-[max(40rem,70%)]" : "md:w-(--peek-width)",
            ),
      )}
    >
      {/* The edge to drag, on the panel's own left edge: a pointer's alone,
          as the agent window's edges are, Full size being the keyboard's way
          to more room. A double click puts the panel back at its own width. */}
      {!peekFull && (
        <div
          aria-hidden="true"
          data-testid="peek-resize"
          onPointerDown={beginDrag}
          onPointerMove={trackDrag}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onLostPointerCapture={endDrag}
          onDoubleClick={() => setTracePrefs({ peekWidth: null })}
          className={cn(
            "absolute inset-y-0 left-0 z-10 hidden w-1.5 cursor-ew-resize touch-none md:block",
            "hover:bg-foreground/10",
            dragged !== null && "bg-foreground/10",
          )}
        />
      )}
      <div className="grid shrink-0 gap-1.5 border-b border-border p-3.5">
        <div className="flex min-h-9 min-w-0 items-center gap-2">
          <ListTree className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          <h2 id={titleId} className="min-w-0 truncate text-base font-medium">
            {summary ? summary.root.agent : "Trace"}
          </h2>
          <code className="shrink-0 text-xs text-muted-foreground" title={shown}>
            {shortId(shown)}
          </code>
          <CopyButton value={shown} label="the run's id" />
          <div className="ml-auto flex shrink-0 items-center gap-0.5">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon" asChild>
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
              label={peekFull ? "Exit full size" : "Full size"}
              onClick={() => setTracePrefs({ peekFull: !peekFull })}
              className="max-md:hidden"
            >
              {peekFull ? <Minimize2 /> : <Maximize2 />}
            </PanelAction>
            <PanelAction label="Close" onClick={close}>
              <XIcon />
            </PanelAction>
          </div>
        </div>
        {trace && <TraceMeta trace={trace} organization={organization} slug={slug} />}
      </div>
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
    </div>
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
