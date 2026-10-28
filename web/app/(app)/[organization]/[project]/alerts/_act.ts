"use client";

import { useEffect, useRef } from "react";
import { toast } from "sonner";

import type { ActionResult, Refused } from "@/lib/action-result";
import { ALERTS_HEADING } from "@/lib/alerts";
import { useAction } from "@/lib/use-action";

/** What an act answers: refused as `gone` where what it acted on is gone, and
 *  the page is drawn again without it. */
export type Answer = ActionResult | (Refused & { gone: true });

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
    if (r.ok || !("gone" in r)) return false;
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
  const { pending, error, run } = useAction();
  const { moved, gone } = useLanding(landing);
  const act = (action: () => Promise<Answer>, takesItAway = false) =>
    run(async () => {
      const r = await action();
      // A toast said what is gone, and handed the focus on.
      if (gone(r)) return { ok: true };
      if (r.ok) moved.current = takesItAway;
      return r;
    });
  return { pending, error, act, movedRef: moved };
}
