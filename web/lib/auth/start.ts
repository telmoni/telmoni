import "server-only";

import { type NextRequest, NextResponse } from "next/server";

import { safeReturnTo } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { newProof } from "@/lib/auth/pkce";
import { PKCE_COOKIE, cookieOpts, getSession, sealPkce } from "@/lib/auth/session";
import { logger } from "@/lib/logger";

// One bucket for every way into a sign-in, the two doors and "Continue
// with…": each is the same call to auth, so each spends the same ceiling.
const AUTH_START_CEILING = { limit: 60, windowMs: 60_000 };

/** A page asked for too often, in plain text: a browser lands on it. */
export function tooManyAttempts(retryAfterSecs: number, what = "sign-in"): NextResponse {
  return new NextResponse(`Too many ${what} attempts. Try again in a minute.`, {
    status: 429,
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "retry-after": String(retryAfterSecs),
      "cache-control": "no-store, max-age=0",
    },
  });
}

/** The 429 for a sign-in start past its ceiling, or `null` with room left. */
export async function pastStartCeiling(request: NextRequest): Promise<NextResponse | null> {
  const retryAfter = await rateLimitRetryAfter(clientKey(request, "auth:start"), AUTH_START_CEILING);
  return retryAfter === null ? null : tooManyAttempts(retryAfter);
}

export function signInUnavailable(): NextResponse {
  return new NextResponse("Sign-in is unavailable right now. Try again in a minute.", {
    status: 503,
    headers: { "content-type": "text/plain; charset=utf-8", "retry-after": "60" },
  });
}

/** On to the provider, with the sign-in's proof sealed for the callback. */
export function toProvider(authUrl: string, sealed: string): NextResponse {
  const response = NextResponse.redirect(authUrl);
  response.headers.set("cache-control", "no-store, max-age=0");
  response.cookies.set({ name: PKCE_COOKIE, value: sealed, ...cookieOpts(600) });
  return response;
}

/** A door into a sign-in: back where the person was going if they hold a
 *  session, else a fresh state and proof, sealed, and on to auth. */
export async function startSignIn(
  request: NextRequest,
  opts: { signUp?: boolean; loginHint?: string } = {},
): Promise<NextResponse> {
  const returnTo = safeReturnTo(
    request.nextUrl.searchParams.get("returnTo") ?? "/console",
    request.nextUrl,
  );
  if (await getSession()) return NextResponse.redirect(new URL(returnTo, request.url));

  // Charged after the signed-in short-circuit: that answer costs nothing
  // upstream, while everything below it is a round trip to auth and the
  // identity provider per hit.
  const refused = await pastStartCeiling(request);
  if (refused) return refused;

  const state = crypto.randomUUID();
  const proof = await newProof();
  let authUrl: string;
  try {
    authUrl = await getAuthorizeUrl(state, {
      codeChallenge: proof.challenge,
      nonce: proof.nonce,
      ...(opts.signUp ? { signUp: true } : {}),
      ...(opts.loginHint ? { loginHint: opts.loginHint } : {}),
    });
  } catch (err) {
    logger.error({ err }, "auth start failed");
    return signInUnavailable();
  }
  const sealed = await sealPkce({ state, verifier: proof.verifier, nonce: proof.nonce, returnTo });
  return toProvider(authUrl, sealed);
}
