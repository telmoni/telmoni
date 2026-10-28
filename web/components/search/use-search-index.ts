"use client";

import { useEffect, useRef, useState } from "react";

import { SESSION_EXPIRED } from "@/lib/action-result";
import { problemWords } from "@/lib/api/problem";
import type { SearchIndex } from "@/lib/search";

const UNREAD = "We couldn't read this project's resources just now.";

export function useSearchIndex(open: boolean, projectId?: string) {
  const [index, setIndex] = useState<SearchIndex | null>(null);
  const [indexError, setIndexError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const indexedProject = useRef<string | null>(null);

  useEffect(() => {
    if (!open || !projectId) return;
    if (indexedProject.current === projectId) return;
    indexedProject.current = projectId;
    // A read, from the index's Route Handler (`app/api/v1/search/index`),
    // dropped when the palette closes or moves to another project before it
    // lands — and then forgotten, so the next opening asks again.
    const abort = new AbortController();
    let settled = false;
    setLoading(true);
    // Drop the last project's results before asking for this one's. This
    // does not remount across a navigation, so index outlives the project it
    // was fetched for: dropping it prevents showing previous project's items.
    setIndex(null);
    setIndexError(null);
    fetch(`/api/v1/search/index?project=${encodeURIComponent(projectId)}`, {
      signal: abort.signal,
    })
      .then(async (res) => {
        const answer: unknown = await res.json().catch(() => null);
        if (!res.ok || !answer || typeof answer !== "object") {
          // A lapsed session is turned away by the console's proxy before
          // the handler runs, in a word meant for code; say it as the
          // handler would have.
          setIndexError(res.status === 401 ? SESSION_EXPIRED : problemWords(answer) || UNREAD);
          indexedProject.current = null;
          return;
        }
        setIndex(answer as SearchIndex);
      })
      .catch(() => {
        if (abort.signal.aborted) return;
        setIndexError(UNREAD);
        indexedProject.current = null;
      })
      .finally(() => {
        settled = true;
        if (!abort.signal.aborted) setLoading(false);
      });
    return () => {
      abort.abort();
      if (!settled) indexedProject.current = null;
    };
  }, [open, projectId]);

  return { index, indexError, loading };
}
