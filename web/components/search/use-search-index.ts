"use client";

import { useEffect, useRef, useState } from "react";
import { searchIndexAction, type SearchIndex } from "@/app/(app)/search-actions";

export function useSearchIndex(open: boolean, projectId?: string) {
  const [index, setIndex] = useState<SearchIndex | null>(null);
  const [indexError, setIndexError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const indexedProject = useRef<string | null>(null);

  useEffect(() => {
    if (!open || !projectId) return;
    if (indexedProject.current === projectId) return;
    indexedProject.current = projectId;
    let live = true;
    setLoading(true);
    // Drop the last project's results before asking for this one's. This
    // does not remount across a navigation, so index outlives the project it
    // was fetched for: dropping it prevents showing previous project's items.
    setIndex(null);
    setIndexError(null);
    void searchIndexAction(projectId)
      .then((answer) => {
        if (!live) return;
        if ("error" in answer) {
          setIndex(null);
          setIndexError(answer.error);
          indexedProject.current = null;
          return;
        }
        setIndex(answer);
      })
      .catch(() => {
        if (!live) return;
        setIndexError("We couldn't read this project's resources just now.");
        indexedProject.current = null;
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [open, projectId]);

  return { index, indexError, loading };
}
