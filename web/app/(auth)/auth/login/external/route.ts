import { type NextRequest, NextResponse } from "next/server";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { PKCE_COOKIE, cookieOpts, sealPkce, unsealPkce } from "@/lib/auth/session";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

export const dynamic = "force-dynamic";

// The same bucket as the two doors: this is the same call to auth.
const AUTH_START_CEILING = { limit: 60, windowMs: 60_000 };

// The sign-in page's "Continue with…". The door already sealed the state
// and where to land, so this carries the same sign-in on to the external
// provider, and marks it as the provider's so the callback spends the code
// there. A visit with no sign-in in progress goes back through the door.
export async function GET(request: NextRequest) {
  const sealed = request.cookies.get(PKCE_COOKIE)?.value;
  const pkce = sealed ? await unsealPkce(sealed) : null;
  if (!pkce) {
    return NextResponse.redirect(new URL("/auth/login", env.AUTH_URL));
  }

  const retryAfter = await rateLimitRetryAfter(
    clientKey(request, "auth:start"),
    AUTH_START_CEILING,
  );
  if (retryAfter !== null) {
    return new NextResponse("Too many sign-in attempts. Try again in a minute.", {
      status: 429,
      headers: {
        "content-type": "text/plain; charset=utf-8",
        "retry-after": String(retryAfter),
        "cache-control": "no-store, max-age=0",
      },
    });
  }

  let authUrl: string;
  try {
    authUrl = await getAuthorizeUrl(pkce.state, { provider: "external" });
  } catch (err) {
    logger.error({ err }, "auth start failed");
    return new NextResponse("Sign-in is unavailable right now. Try again in a minute.", {
      status: 503,
      headers: { "content-type": "text/plain; charset=utf-8", "retry-after": "60" },
    });
  }

  const response = NextResponse.redirect(authUrl);
  response.headers.set("cache-control", "no-store, max-age=0");
  response.cookies.set({
    name: PKCE_COOKIE,
    value: await sealPkce({ ...pkce, provider: "external" }),
    ...cookieOpts(600),
  });
  return response;
}
