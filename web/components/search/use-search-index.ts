"use client";

import { useEffect, useRef, useState } from "react";

import { readRoute } from "@/components/route-read";
import { type RouteRead, refusalWords } from "@/lib/route-read";
import { SearchIndexSchema, type SearchIndex } from "@/lib/search";

const UNREAD = "We couldn't read this project's resources just now.";

/** The index of the project the palette is opened on, read from its Route
 *  Handler (`app/api/v1/search/index`) when the palette opens, dropped when it
 *  closes or moves to another project before the read lands — and then
 *  forgotten, so the next opening asks again. Keyed by project, so another
 *  project's index never shows under this one's. */
export function useSearchIndex(open: boolean, projectId?: string) {
  const [read, setRead] = useState<{
    projectId: string;
    answer: RouteRead<SearchIndex> | null;
  } | null>(null);
  const asked = useRef<string | null>(null);

  useEffect(() => {
    if (!open || !projectId) return;
    if (asked.current === projectId) return;
    asked.current = projectId;
    const abort = new AbortController();
    let settled = false;
    setRead({ projectId, answer: null });
    void readRoute(
      `/api/v1/search/index?project=${encodeURIComponent(projectId)}`,
      SearchIndexSchema,
      { signal: abort.signal },
    ).then((answer) => {
      settled = true;
      if (abort.signal.aborted) return;
      if (answer.kind !== "ok") asked.current = null;
      setRead({ projectId, answer });
    });
    return () => {
      abort.abort();
      if (!settled) asked.current = null;
    };
  }, [open, projectId]);

  const mine = read?.projectId === projectId ? read : null;
  return {
    index: mine?.answer?.kind === "ok" ? mine.answer.value : null,
    indexError:
      mine?.answer && mine.answer.kind !== "ok" ? refusalWords(mine.answer, UNREAD) : null,
    loading: mine !== null && mine.answer === null,
  };
}
