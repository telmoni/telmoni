// The session as the proxy passes it on: renewed there, once its bearer is
// about to lapse, because the proxy is the one place a request can both write
// the cookie the browser keeps and hand the renewed one to everything the
// request renders. A page cannot write it, and a refresh token is spent
// once: presented again past auth's thirty seconds' grace, it ends the
// session (`lib/auth/session.ts`, `getSession`).

import {
  refreshSessionTokens,
  sealSession,
  unsealSession,
  type SessionData,
} from "@/lib/auth/session";
import { isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { logger } from "@/lib/logger";

/** How near its bearer's end a session is renewed. Under the heartbeat's two
 *  minutes, so a console tab's own renewal comes first. */
const RENEW_WITHIN_MS = 60_000;

/** How long a renewal's answer is kept for the requests that left the browser
 *  with the old cookie before the new one reached it: long enough for a
 *  page's last prefetch, well inside auth's grace. */
const ANSWER_KEPT_MS = 10_000;

/** How long no answer is kept: the burst it came in shares it, and the next
 *  request asks again inside auth's grace — auth may have spent the token
 *  before its answer was lost, and an ask past the grace would read as a
 *  stolen copy and end the session. */
const NO_ANSWER_KEPT_MS = 2_000;

/** What a request does with the session cookie it came with. */
export type SettledSession =
  /** No cookie, or one that does not unseal: nobody is signed in. */
  | { kind: "none" }
  /** Signed in, and the cookie goes on as it came: its bearer has time
   *  left, the console already ended it (the pages sign that out, saying
   *  how), or auth did not answer the renewal, which is asked again a moment
   *  later. */
  | { kind: "kept" }
  /** Renewed: the new cookie goes to the page and to the browser alike. */
  | { kind: "renewed"; sealed: string }
  /** Auth refused the renewal: the session is over, and its cookie goes. */
  | { kind: "ended" };

// Renewals under way, by the refresh token each spends. A page's requests
// arrive together — the document, its prefetches, the event stream — each
// with the same cookie, and would otherwise each spend the token for a pair
// of its own, and hand the browser as many cookies.
const renewals = new Map<string, Promise<SettledSession>>();

export async function settleSession(cookie: string | undefined): Promise<SettledSession> {
  const session = await unsealSession(cookie);
  if (!session) return { kind: "none" };
  if (Date.now() + RENEW_WITHIN_MS < session.expiresAt) return { kind: "kept" };
  if (await isSessionBlacklisted(sessionEndKey(session))) return { kind: "kept" };
  const token = session.refreshToken;
  if (!token) return { kind: "ended" };

  const underway = renewals.get(token);
  if (underway) return underway;
  const renewal = renew(session)
    // Every request the console answers passes here, so a renewal that
    // throws leaves the cookie as it came rather than failing the request.
    .catch((err: unknown): SettledSession => {
      logger.error(
        { error: err instanceof Error ? err.message : String(err) },
        "proxy: session renewal failed",
      );
      return { kind: "kept" };
    });
  renewals.set(token, renewal);
  void renewal.then((settled) => {
    const keptFor = settled.kind === "kept" ? NO_ANSWER_KEPT_MS : ANSWER_KEPT_MS;
    setTimeout(() => {
      if (renewals.get(token) === renewal) renewals.delete(token);
    }, keptFor);
  });
  return renewal;
}

async function renew(session: SessionData): Promise<SettledSession> {
  const renewal = await refreshSessionTokens(session);
  switch (renewal.kind) {
    case "renewed":
      return { kind: "renewed", sealed: await sealSession(renewal.session) };
    case "refused":
      return { kind: "ended" };
    case "unreachable":
      return { kind: "kept" };
  }
}

/** For tests: forget every renewal, as a fresh process would. */
export function forgetRenewalsForTest(): void {
  renewals.clear();
}
