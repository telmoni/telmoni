import { type NextRequest, NextResponse } from "next/server";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { PKCE_COOKIE, cookieOpts, getSession, sealPkce } from "@/lib/auth/session";
import { safeReturnTo } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { logger } from "@/lib/logger";

export const dynamic = "force-dynamic";

const AUTH_START_CEILING = { limit: 60, windowMs: 60_000 };

function tooManyStarts(retryAfterSecs: number): NextResponse {
  return new NextResponse("Too many sign-in attempts. Try again in a minute.", {
    status: 429,
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "retry-after": String(retryAfterSecs),
      "cache-control": "no-store, max-age=0",
    },
  });
}

export async function GET(request: NextRequest) {
  const returnTo = safeReturnTo(
    request.nextUrl.searchParams.get("returnTo") ?? "/console",
    request.nextUrl,
  );

  const session = await getSession();
  if (session) {
    return NextResponse.redirect(new URL(returnTo, request.url));
  }

  // Charged after the signed-in short-circuit: that answer costs nothing
  // upstream, while everything below it is a round trip to auth and the
  // identity provider per hit. One bucket for both doors — they spend the
  // same call.
  const retryAfter = await rateLimitRetryAfter(
    clientKey(request, "auth:start"),
    AUTH_START_CEILING,
  );
  if (retryAfter !== null) return tooManyStarts(retryAfter);

  const state = crypto.randomUUID();
  const hint = loginHint(request.nextUrl.searchParams.get("email"));
  let authUrl: string;
  try {
    authUrl = await getAuthorizeUrl(state, {
      signUp: true,
      ...(hint ? { loginHint: hint } : {}),
    });
  } catch (err) {
    logger.error({ err }, "auth start failed");
    return new NextResponse("Sign-in is unavailable right now. Try again in a minute.", {
      status: 503,
      headers: { "content-type": "text/plain; charset=utf-8", "retry-after": "60" },
    });
  }

  const sealed = await sealPkce({ state, returnTo });
  const response = NextResponse.redirect(authUrl);
  response.headers.set("cache-control", "no-store, max-age=0");
  response.cookies.set({
    name: PKCE_COOKIE,
    value: sealed,
    ...cookieOpts(600),
  });
  return response;
}

function loginHint(raw: string | null): string | undefined {
  const value = raw?.trim() ?? "";
  if (value.length === 0 || value.length > 254) return undefined;
  return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value) ? value : undefined;
}
