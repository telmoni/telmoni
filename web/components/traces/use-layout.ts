"use client";

import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "next/navigation";

import {
  VIEW_PARAM,
  choiceOfDefault,
  choiceOfLayout,
  edited,
  parseLayoutUi,
  resolveLayout,
  sameLayout,
  withLayoutParams,
  type LayoutChoice,
  type LayoutOptions,
  type LayoutSource,
} from "@/lib/layout";
import type { SessionLayoutSetting, TraceLayout } from "@/lib/server/data";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";

// The layout a session's page or a trace's stands in, as the header's
// controls and the page's body both read it: the link's, the browser's or
// the project's (`lib/layout.ts`). A change is kept in the browser as the
// person's last choice, and the link's part of the URL is rewritten in
// place, with the browser's own history, so a copied link holds a saved
// layout and never a half-made cut.

export type LayoutPage = {
  /** The project's saved layouts, or `null` where they could not be read. */
  layouts: readonly TraceLayout[] | null;
  /** What the project opens in, or `null` where it could not be read. */
  project: SessionLayoutSetting | null;
};

export type Layout = {
  choice: LayoutChoice;
  options: LayoutOptions;
  source: LayoutSource;
  /** Whether the options have moved off what they began from. */
  isEdited: boolean;
  /** The saved layout the options began from, if one. */
  current: TraceLayout | undefined;
  setOptions: (patch: Partial<LayoutOptions>) => void;
  applyLayout: (layout: TraceLayout) => void;
  /** Back to what the options began from: the saved layout, or the default. */
  reset: () => void;
};

/** A choice applied on this page, with the link's part of the URL as it
 *  stood then and as the choice wrote it. */
type Applied = { before: string | null; wrote: string | null; choice: LayoutChoice };

const sameChoice = (a: LayoutChoice, b: LayoutChoice) =>
  a.view === b.view && sameLayout(a.options, b.options);

/** The link's part of the URL, written in place. ⚠ A fresh state object,
 *  never `window.history.state`: Next copies its own into one and follows
 *  the URL, and passes one that is already its own straight through, with
 *  the router — and `useSearchParams` — none the wiser. */
function rewrite(choice: LayoutChoice, isEdited: boolean): string | null {
  const query = withLayoutParams(window.location.search, choice, isEdited);
  window.history.replaceState({}, "", `${window.location.pathname}${query}`);
  return new URLSearchParams(query).get(VIEW_PARAM);
}

export function useLayout({ layouts, project }: LayoutPage): Layout {
  const search = useSearchParams();
  const prefs = useTracePrefs();
  const known = layouts ?? [];
  const urlView = search.get(VIEW_PARAM);
  const url = parseLayoutUi({ [VIEW_PARAM]: urlView ?? undefined });

  // A choice applied here stands while the URL still reads as it did before
  // the rewrite: Next follows a rewrite in a transition of its own, and a
  // render in between would read the old link over the choice. Once the URL
  // has moved on — to what was written, or to a link or a save's redirect —
  // the URL decides again.
  const [applied, setApplied] = useState<Applied | null>(null);
  const stale = applied !== null && applied.before !== applied.wrote && applied.before === urlView;
  // Let go once the URL has moved on, as state is adjusted on a change in
  // what it followed: a Back to the old link later is the link's again.
  if (applied && !stale) setApplied(null);

  const resolved = stale
    ? { choice: applied.choice, source: "browser" as const }
    : resolveLayout(url, prefs.layout, project, known);
  const { choice, source } = resolved;
  const isEdited = edited(choice, known);

  // A link's layout becomes the person's last choice, as a saved view's
  // columns become the browser's on any click: once per link, as the URL
  // changes, read through a ref so the effect binds to the URL alone.
  const latest = useRef({ stale, source, choice, kept: prefs.layout });
  useEffect(() => {
    latest.current = { stale, source, choice, kept: prefs.layout };
  });
  useEffect(() => {
    const now = latest.current;
    if (now.stale || now.source !== "link") return;
    if (now.kept && sameChoice(now.kept, now.choice)) return;
    setTracePrefs({ layout: now.choice });
  }, [urlView]);

  const apply = (next: LayoutChoice) => {
    const wrote = rewrite(next, edited(next, known));
    setApplied({ before: urlView, wrote, choice: next });
    setTracePrefs({ layout: next });
  };
  const from = choice.view ? known.find((layout) => layout.id === choice.view) : undefined;

  return {
    choice,
    options: choice.options,
    source,
    isEdited,
    current: from,
    setOptions: (patch) => apply({ view: choice.view, options: { ...choice.options, ...patch } }),
    applyLayout: (layout) => apply(choiceOfLayout(layout)),
    reset: () => apply(from ? choiceOfLayout(from) : choiceOfDefault()),
  };
}
