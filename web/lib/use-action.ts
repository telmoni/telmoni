"use client";

import { unstable_rethrow } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import type { ActionResult } from "@/lib/action-result";

export const NETWORK_ERROR = "Network error. Try again.";

/** Runs a Server Action in a transition: a refusal held as `error` for the
 *  control to say, the action's redirect left to the router, and anything
 *  else said as a network error. */
export function useAction() {
  const [pending, start] = useTransition();
  const [error, setError] = useState<string | null>(null);

  function run<T extends object>(
    action: () => Promise<ActionResult<T>>,
    then?: (done: { ok: true } & T) => void,
  ) {
    start(async () => {
      setError(null);
      try {
        const r = await action();
        if (!r.ok) {
          setError(r.error);
          return;
        }
        then?.(r);
      } catch (err) {
        // The action's redirect rejects the call, and belongs to the router.
        unstable_rethrow(err);
        setError(NETWORK_ERROR);
      }
    });
  }

  return { pending, error, setError, run };
}

/** As `useAction`, for a control with no room to say a refusal: it is a
 *  toast, and so is `done` when given. */
export function useToastAction() {
  const [pending, start] = useTransition();

  function run<T extends object>(
    action: () => Promise<ActionResult<T>>,
    done?: string | ((r: { ok: true } & T) => void),
  ) {
    start(async () => {
      try {
        const r = await action();
        if (!r.ok) {
          toast.error(r.error);
          return;
        }
        if (typeof done === "string") toast.success(done);
        else done?.(r);
      } catch (err) {
        unstable_rethrow(err);
        toast.error(NETWORK_ERROR);
      }
    });
  }

  return { pending, run };
}
