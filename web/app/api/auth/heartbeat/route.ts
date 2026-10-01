import { type NextRequest, NextResponse } from "next/server";

import { SESSION_ENDED } from "@/lib/auth/heartbeat";
import {
  SESSION_COOKIE,
  SESSION_TTL_SECONDS,
  cookieOpts,
  getSession,
  refreshSessionTokens,
  sealSession,
  sessionEndedAlready,
} from "@/lib/auth/session";
import { isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";

// ⚠ **Refused cross-site before anything else, because the 401 below DELETES
// THE COOKIE.** A page on another origin can auto-submit a form here; the
// browser withholds the session cookie (SameSite=Lax) so the handler sees no
// session and answers 401 with a Set-Cookie that clears it — and the browser
// applies that to the real cookie. A forced logout from any site, with no
// credential involved. Server Actions get an Origin check from Next for free;
// a Route Handler gets nothing, so this is that check. Absent header, or
// `same-origin`/`none`, is the console's own fetch and a direct load.
export async function POST(request: NextRequest) {
  const secFetchSite = request.headers.get("sec-fetch-site");
  if (secFetchSite && secFetchSite !== "same-origin" && secFetchSite !== "none") {
    return NextResponse.json({ ok: false, error: "forbidden" }, { status: 403 });
  }

  // A session the console ended is told apart from one that ran out, so the
  // heartbeat can sign it out to the home page rather than to sign-in. Read
  // first: `getSession` deletes a blacklisted cookie as it refuses it.
  const ended = await sessionEndedAlready();
  const session = await getSession();
  if (!session) {
    const response = NextResponse.json(
      { ok: false, error: ended ? SESSION_ENDED : "unauthenticated" },
      { status: 401 },
    );
    response.cookies.delete(SESSION_COOKIE);
    return response;
  }

  if (await isSessionBlacklisted(sessionEndKey(session))) {
    const response = NextResponse.json(
      { ok: false, error: SESSION_ENDED },
      { status: 401 },
    );
    response.cookies.delete(SESSION_COOKIE);
    return response;
  }

  let currentSession = session;
  const NEAR_EXPIRY_MS = 120_000;
  const isNearExpiry =
    currentSession.refreshToken &&
    currentSession.expiresAt > 0 &&
    Date.now() + NEAR_EXPIRY_MS >= currentSession.expiresAt;

  if (isNearExpiry) {
    // The refresh names the session row, so a revocation from another device
    // surfaces here as a refused grant rather than through a call of its own.
    const refreshed = await refreshSessionTokens(currentSession);
    if (!refreshed) {
      const response = NextResponse.json(
        { ok: false, error: "refresh_failed" },
        { status: 401 },
      );
      response.cookies.delete(SESSION_COOKIE);
      return response;
    }
    currentSession = refreshed;
  }

  const sealed = await sealSession(currentSession);
  const response = NextResponse.json({
    ok: true,
    expiresAt: currentSession.expiresAt,
  });
  response.cookies.set({
    name: SESSION_COOKIE,
    value: sealed,
    ...cookieOpts(SESSION_TTL_SECONDS),
  });

  return response;
}
