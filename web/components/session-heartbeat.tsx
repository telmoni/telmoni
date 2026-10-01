"use client";

import { useEffect, useRef } from "react";
import { useRouter } from "next/navigation";

import { afterRefusedHeartbeat } from "@/lib/auth/heartbeat";

const RETRY_MS = 30_000;

const REQUEST_TIMEOUT_MS = 10_000;

interface SessionHeartbeatProps {
  expiresAt: number;
  needsReseal?: boolean;
}

export function SessionHeartbeat({
  expiresAt: initialExpiresAt,
  needsReseal,
}: SessionHeartbeatProps) {
  const router = useRouter();
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
      try {
        const res = await fetch("/api/auth/heartbeat", {
          method: "POST",
          headers: {
            "content-type": "application/json",
          },
          signal: abort.signal,
        });
        if (unmounted) return;
        if (res.status === 401) {
          const where = afterRefusedHeartbeat(await res.json().catch(() => null));
          if (where === "/auth/logout") {
            window.location.replace(where);
          } else {
            router.push(where);
          }
          return;
        }
        if (res.ok) {
          const data = (await res.json().catch(() => null)) as {
            ok?: boolean;
            expiresAt?: number;
          } | null;
          if (data?.expiresAt) {
            expiresAtRef.current = data.expiresAt;
            scheduleNext();
            return;
          }
        }
      } catch {
      } finally {
        clearTimeout(cutoff);
        inFlight = false;
      }
      if (!unmounted) scheduleNext(RETRY_MS);
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

    // If Next.js rendered a Server Component where cookies are read-only,
    // getSession() flags needsReseal=true so this client heartbeat immediately
    // asks /api/auth/heartbeat (a Route Handler) to persist the updated cookie.
    const requiresImmediatePing =
      needsReseal ||
      (expiresAtRef.current > 0 &&
        Date.now() + 90_000 >= expiresAtRef.current);

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
  }, [initialExpiresAt, needsReseal, router]);

  return null;
}
