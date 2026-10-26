"use client";

import { unstable_rethrow } from "next/navigation";
import { useEffect, useRef, useState, useTransition } from "react";

/** Runs an action, holding its error for the row to say. Once one that takes
 *  its control away has, the focus goes to the element `landing` names rather
 *  than falling to the page's body. */
export function useAct(landing?: string) {
  const [pending, start] = useTransition();
  const [error, setError] = useState<string | null>(null);
  // Set before the refresh that draws the page without the control.
  const moved = useRef(false);
  useEffect(
    () => () => {
      if (moved.current && landing && document.activeElement === document.body) {
        document.getElementById(landing)?.focus({ preventScroll: true });
      }
    },
    [landing],
  );
  const act = (run: () => Promise<{ error: string | null }>, takesItAway = false) =>
    start(async () => {
      setError(null);
      try {
        const r = await run();
        if (r.error) setError(r.error);
        else moved.current = takesItAway;
      } catch (err) {
        unstable_rethrow(err);
        setError("Network error. Try again.");
      }
    });
  return { pending, error, act, movedRef: moved };
}
