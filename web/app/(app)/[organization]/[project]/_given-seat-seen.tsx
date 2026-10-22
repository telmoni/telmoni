"use client";

import { useEffect } from "react";

import { markSeatSeenAction } from "@/app/(app)/account/notifications/seat-actions";
import { useGivenSeats, useRemoveGivenSeat } from "@/lib/store";

// A seat given is seen once its project is open, however it was reached: the
// mail's link, the sidebar, a new tab. The bell's Open marks it before it
// leads here, so it is waiting no longer by then.
export function GivenSeatSeen({ projectId }: { projectId: string }) {
  const waiting = (useGivenSeats() ?? []).some((seat) => seat.projectId === projectId);
  const removeGivenSeat = useRemoveGivenSeat();

  useEffect(() => {
    if (!waiting) return;
    markSeatSeenAction(projectId).then(
      (r) => {
        if (!r.error) removeGivenSeat(projectId);
      },
      // Left waiting, as a refused mark leaves it: the bell still offers it.
      () => {},
    );
  }, [waiting, projectId, removeGivenSeat]);

  return null;
}
