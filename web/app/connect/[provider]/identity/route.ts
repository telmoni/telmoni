import { type NextRequest, NextResponse } from "next/server";

import { readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { tooManyAttempts } from "@/lib/auth/start";
import { CONNECT_COOKIE, cookieOpts, getSession, sealConnect } from "@/lib/auth/session";
import { type LinkRefusal, accountSlackPath, isOAuthProvider } from "@/lib/connect";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { AuthorizeSchema } from "@/lib/server/entities/connectors";
import { accountHeaders } from "@/lib/server/entities/identity-context";

// Each press mints a state row in the service, so the ceiling the install's
// start carries.
const LINK_START_CEILING = { limit: 30, windowMs: 60_000 };

// A person's own account at the vendor, linked from their account settings
// so that a press of a button on a notice there acts as them. A GET Route
// Handler for the start route's reasons: a lapsed session comes back here
// through login, and the CSP's `form-action` governs no navigation. It names
// no project: the service keeps the state as the person's own, and the
// vendor sends them back to the install's registered callback.
export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ provider: string }> },
) {
  const { provider } = await params;
  if (!isOAuthProvider(provider)) {
    return NextResponse.redirect(new URL("/console", env.AUTH_URL));
  }

  const session = await getSession();
  if (!session) {
    const login = new URL("/auth/login", env.AUTH_URL);
    login.searchParams.set("returnTo", request.nextUrl.pathname);
    return NextResponse.redirect(login);
  }

  const retryAfter = await rateLimitRetryAfter(
    clientKey(request, "connect:identity"),
    LINK_START_CEILING,
  );
  if (retryAfter !== null) return tooManyAttempts(retryAfter, "connection");

  const back = (refusal: LinkRefusal) =>
    NextResponse.redirect(new URL(accountSlackPath(refusal), env.AUTH_URL));

  const headers = await accountHeaders();
  if (!headers) return back("unavailable");
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/notifications/${provider}/identities/authorize`,
    { method: "POST", headers },
  );
  if (!res) return back("unavailable");
  if (!res.ok) {
    logger.warn({ provider, status: res.status }, "account link refused to start");
    return back("start");
  }
  const parsed = await readBody(res, AuthorizeSchema);
  if (!parsed) return back("start");

  const sealed = await sealConnect({ purpose: "identity", state: parsed.state, provider });
  const response = NextResponse.redirect(parsed.url);
  response.headers.set("cache-control", "no-store, max-age=0");
  response.cookies.set({
    name: CONNECT_COOKIE,
    value: sealed,
    ...cookieOpts(600),
  });
  return response;
}
