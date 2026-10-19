"use client";

import { useEffect, useRef } from "react";

import { afterRefusedHeartbeat } from "@/lib/auth/heartbeat";

const RETRY_MS = 30_000;

/** After a renewal nobody answered — a 503, auth silent, or the ping cut off
 *  — which auth may have made all the same: asked again well inside the
 *  thirty seconds auth lets a spent refresh token come back in, past which
 *  the ask reads as a stolen copy and ends the session. That long, never
 *  later: not held to the schedule's mark before expiry. */
const UNANSWERED_RETRY_MS = 5_000;

/** How many quick asks the grace holds. Past it a spent token is lost
 *  whatever is asked, and an auth that stays down is asked at the usual
 *  pace rather than every few seconds from every open tab. */
const UNANSWERED_RETRIES = 4;

/** Past the route's own ten seconds on auth, so its 503 comes back rather
 *  than this cutoff. */
const REQUEST_TIMEOUT_MS = 15_000;

interface SessionHeartbeatProps {
  expiresAt: number;
}

export function SessionHeartbeat({ expiresAt: initialExpiresAt }: SessionHeartbeatProps) {
  const expiresAtRef = useRef(initialExpiresAt);
  const timerRef = useRef<NodeJS.Timeout | null>(null);

  useEffect(() => {
    expiresAtRef.current = initialExpiresAt;
    let unmounted = false;
    // ⚠ **Scoped to this effect run, not a ref that outlives it.** As a ref
    // it wedged the heartbeat for good: `expiresAt` moving while a ping is in
    // flight — which is what a successful ping plus the realtime listener's
    // `router.refresh()` produces — re-runs this effect, the new closure's
    // `pingHeartbeat` returns early on the OLD closure's flag and schedules
    // nothing, and the old ping's own `if (!unmounted)` then refuses to
    // schedule too. Neither reaches `scheduleNext`, no timer exists, and the
    // session expires unrefreshed until something remounts this.
    //
    // A closure-local flag still does the only job it had — no two
    // overlapping pings from one run — and teardown discards it.
    let inFlight = false;
    let unansweredInRow = 0;

    async function pingHeartbeat() {
      if (inFlight || unmounted) return;
      inFlight = true;
      // Without a cutoff, a request that never settles strands `inFlight` at
      // true: every later ping returns early, nothing reaches scheduleNext,
      // and the session expires with no warning until this remounts. A
      // rejection is fine — the catch below falls through to the retry. This
      // is an AbortController rather than AbortSignal.timeout because the
      // suite runs on fake timers, which drive setTimeout but not the latter.
      const abort = new AbortController();
      const cutoff = setTimeout(() => abort.abort(), REQUEST_TIMEOUT_MS);
      let unanswered = false;
      try {
        const res = await fetch("/api/v1/auth/heartbeat", {
          method: "POST",
          headers: {
            "content-type": "application/json",
          },
          signal: abort.signal,
        });
        if (unmounted) return;
        if (res.status === 401) {
          // Both ways out are Route Handlers, which `router.push` is not for
          // (it fetches a page's payload): a full load runs them and follows
          // where they redirect.
          window.location.replace(afterRefusedHeartbeat(await res.json().catch(() => null)));
          return;
        }
        if (res.ok) {
          const data = (await res.json().catch(() => null)) as {
            ok?: boolean;
            expiresAt?: number;
          } | null;
          if (data?.expiresAt) {
            expiresAtRef.current = data.expiresAt;
            unansweredInRow = 0;
            scheduleNext();
            return;
          }
        }
        unanswered = res.status === 503;
      } catch {
        // Cut off, or never sent: auth may have spent the token all the same.
        unanswered = true;
      } finally {
        clearTimeout(cutoff);
        inFlight = false;
      }
      if (unmounted) return;
      if (unanswered && unansweredInRow < UNANSWERED_RETRIES) {
        unansweredInRow += 1;
        askAgainSoon();
      } else {
        scheduleNext(RETRY_MS);
      }
    }

    function askAgainSoon() {
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        void pingHeartbeat();
      }, UNANSWERED_RETRY_MS);
    }

    function scheduleNext(minDelayMs = 5_000) {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      const exp = expiresAtRef.current;
      if (!exp || exp === 0) return;

      const now = Date.now();
      const targetTime = exp - 90_000;
      const delay = Math.max(minDelayMs, targetTime - now);

      timerRef.current = setTimeout(() => {
        void pingHeartbeat();
      }, delay);
    }

    const requiresImmediatePing =
      expiresAtRef.current > 0 && Date.now() + 90_000 >= expiresAtRef.current;

    if (requiresImmediatePing) {
      void pingHeartbeat();
    } else {
      scheduleNext();
    }

    function onVisibilityOrFocus() {
      if (document.visibilityState === "visible") {
        const exp = expiresAtRef.current;
        if (exp > 0 && Date.now() + 120_000 >= exp) {
          void pingHeartbeat();
        }
      }
    }

    document.addEventListener("visibilitychange", onVisibilityOrFocus);
    window.addEventListener("focus", onVisibilityOrFocus);

    return () => {
      unmounted = true;
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
      document.removeEventListener("visibilitychange", onVisibilityOrFocus);
      window.removeEventListener("focus", onVisibilityOrFocus);
    };
  }, [initialExpiresAt]);

  return null;
}
