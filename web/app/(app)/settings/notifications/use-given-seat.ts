"use client";

import { unstable_rethrow, useRouter } from "next/navigation";
import { useState, useTransition, type MouseEvent } from "react";
import { toast } from "sonner";

import { projectPath } from "@/lib/slug";
import { useRemoveGivenSeat } from "@/lib/store";
import type { GivenSeat } from "@/lib/types/given-seat";

import { markSeatSeenAction } from "./seat-actions";

// A seat given, as the bell's and the page's rows offer it. Open marks it seen
// and only then leads to the project, so the project's first read of `/me`
// cannot put it back in the bell; a click that opens a new tab is the
// browser's, and marks it beside. Dismiss marks it where it stands. Open's
// refusal is said by a toast, its row being left behind; Dismiss's on the row.
export function useGivenSeat(
  seat: GivenSeat,
  {
    onLeave,
    onGone,
  }: {
    /** Before Open leads away: the bell closes. */
    onLeave?: () => void;
    /** Before the row goes, once seen: the focus moves off it. */
    onGone?: () => void;
  } = {},
) {
  const router = useRouter();
  const removeGivenSeat = useRemoveGivenSeat();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();
  const [opening, setOpening] = useState(false);
  // By id, which the project's layout spells as its slug: a rename since the
  // row was drawn still leads there.
  const href = projectPath(seat.organizationSlug, seat.projectId);

  function mark(aloud: boolean, then?: () => void) {
    const say = aloud ? (message: string) => toast.error(message) : setError;
    setError(null);
    start(async () => {
      try {
        const r = await markSeatSeenAction(seat.projectId);
        if (!r.ok) say(r.error);
        else {
          onGone?.();
          removeGivenSeat(seat.projectId);
        }
      } catch (err) {
        unstable_rethrow(err);
        say("Couldn't update that. Try again.");
      }
      // On to the project whatever the mark answered; the row stays unseen.
      then?.();
    });
  }

  function open(e: MouseEvent<HTMLAnchorElement>) {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) {
      mark(true);
      return;
    }
    e.preventDefault();
    setOpening(true);
    onLeave?.();
    mark(true, () => router.push(href));
  }

  return { href, open, dismiss: () => mark(false), error, pending, opening };
}
