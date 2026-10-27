"use client";

import { unstable_rethrow } from "next/navigation";
import { useEffect, useRef, useState, useTransition } from "react";
import { toast } from "sonner";

import { ALERTS_HEADING } from "@/lib/alerts";

/** What an act answers: `gone` where what it acted on is gone, and the page
 *  is drawn again without it. */
export type Answer = { error: string | null; gone?: boolean };

/** Once `moved` is set, the control's going hands the focus to the element
 *  `landing` names, or the page's heading where that went too, rather than
 *  letting it fall to the page's body. */
export function useLanding(landing: string) {
  // Set before the refresh that draws the page without the control.
  const moved = useRef(false);
  useEffect(
    () => () => {
      if (moved.current && document.activeElement === document.body) {
        (document.getElementById(landing) ?? document.getElementById(ALERTS_HEADING))?.focus({
          preventScroll: true,
        });
      }
    },
    [landing],
  );
  /** Whether `r` is refused for what is gone: the refresh takes its row away,
   *  and what the row would say with it, so a toast says it. */
  const gone = (r: Answer) => {
    if (!r.gone || !r.error) return false;
    toast.error(r.error);
    moved.current = true;
    return true;
  };
  return { moved, gone };
}

/** Runs an action, holding its error for the row to say. Once one that takes
 *  its control away has, or one is refused for what is gone, the focus is
 *  handed on (`useLanding`). */
export function useAct(landing: string) {
  const [pending, start] = useTransition();
  const [error, setError] = useState<string | null>(null);
  const { moved, gone } = useLanding(landing);
  const act = (run: () => Promise<Answer>, takesItAway = false) =>
    start(async () => {
      setError(null);
      try {
        const r = await run();
        if (gone(r)) return;
        if (r.error) setError(r.error);
        else moved.current = takesItAway;
      } catch (err) {
        unstable_rethrow(err);
        setError("Network error. Try again.");
      }
    });
  return { pending, error, act, movedRef: moved };
}
