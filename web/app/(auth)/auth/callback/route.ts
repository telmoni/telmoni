import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";
import { exchangeCode } from "@/lib/auth/oidc";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { track } from "@/lib/analytics";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { ACTIVE_ORGANIZATION_COOKIE } from "@/lib/proxy/organization";
import {
  SESSION_COOKIE,
  SESSION_TTL_SECONDS,
  PKCE_COOKIE,
  cookieOpts,
  getSession,
  sealSession,
  unsealPkce,
  type SessionData,
} from "@/lib/auth/session";

const CALLBACK_CEILING = { limit: 30, windowMs: 60_000 };

export async function GET(request: NextRequest) {
  const pkceValue = request.cookies.get(PKCE_COOKIE)?.value;
  if (!pkceValue) {
    const existing = await getSession();
    if (existing) {
      return NextResponse.redirect(new URL("/console", env.AUTH_URL));
    }
    return NextResponse.redirect(new URL("/auth/login", env.AUTH_URL));
  }

  const pkce = await unsealPkce(pkceValue);
  if (!pkce) {
    const existing = await getSession();
    if (existing) {
      const response = NextResponse.redirect(new URL("/console", env.AUTH_URL));
      response.cookies.delete(PKCE_COOKIE);
      return response;
    }
    return NextResponse.redirect(new URL("/auth/login", env.AUTH_URL));
  }

  const returnedState = request.nextUrl.searchParams.get("state");
  if (!returnedState || returnedState !== pkce.state) {
    return NextResponse.redirect(
      new URL("/auth/login?error=callback", env.AUTH_URL),
    );
  }

  const code = request.nextUrl.searchParams.get("code");
  if (!code) {
    const existing = await getSession();
    if (existing) {
      const response = NextResponse.redirect(
        new URL(pkce.returnTo || "/console", env.AUTH_URL),
      );
      response.cookies.delete(PKCE_COOKIE);
      return response;
    }
    return NextResponse.redirect(
      new URL("/auth/login?error=callback", env.AUTH_URL),
    );
  }

  // Every branch above answers from the cookie alone; this is the first one
  // that spends a code exchange at the provider, so it is the one a per-source
  // ceiling guards. A caller with a valid cookie, state and code gets in
  // long before thirty a minute.
  const retryAfter = await rateLimitRetryAfter(
    clientKey(request, "auth:callback"),
    CALLBACK_CEILING,
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

  // Whose code this is — the external provider's, or one auth's own sign-in
  // minted — is what the pending sign-in recorded when the browser left.
  let result;
  try {
    result = await exchangeCode(code, pkce.provider);
  } catch {
    const existing = await getSession();
    if (existing) {
      const response = NextResponse.redirect(
        new URL(pkce.returnTo || "/console", env.AUTH_URL),
      );
      response.cookies.delete(PKCE_COOKIE);
      return response;
    }
    return NextResponse.redirect(
      new URL("/auth/login?error=callback", env.AUTH_URL),
    );
  }

  if (!result.emailVerified) {
    return NextResponse.redirect(
      new URL("/auth/login?error=email-unverified", env.AUTH_URL),
    );
  }

  const sessionData: SessionData = {
    userId: result.userId,
    email: result.email ?? "",
    firstName: result.firstName,
    lastName: result.lastName,
    sessionRowId: null,
    accessToken: result.accessToken,
    sessionId: result.sessionId,
    refreshToken: result.refreshToken,
    expiresAt: Date.now() + result.expiresIn * 1000,
    // The external provider's, for the sign-out that names it; a password
    // sign-in has none.
    idToken: result.idToken ?? null,
    authMethod: result.authMethod ?? null,
  };

  // The first `/me` of this sign-in. Under the bearer it provisions the
  // organization and records the session row this browser is listed under,
  // and answers with that row's id for the cookie to carry — the id a sign-out
  // elsewhere revokes, and the one the blacklist is keyed on.
  //
  // ⚠ **The body is the user agent and nothing else.** Auth describes the
  // person from what the exchange just recorded and refuses an email or name
  // here with a 400. This probe still sent them for a day after that change:
  // every sign-in sealed `sessionRowId: null`, so revoking your own session
  // never signed the console out and no refresh touched the row.
  //
  // ⚠ **A closed sign-up is a sign-in too.** A person with no organization
  // while sign-ups are closed is answered with no organization, and the session
  // is sealed like any other; the (app) layout gives them their account.
  try {
    const probe = await fetchWithTimeout(`${env.SERVER_URL}/me`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${sessionData.accessToken}`,
        "content-type": "application/json",
        "x-service-secret": env.SERVICE_SECRET,
        "x-request-id": crypto.randomUUID(),
      },
      body: JSON.stringify({ userAgent: request.headers.get("user-agent") }),
    });
    if (probe.ok) {
      const answered = z
        .object({ sessionRowId: z.string() })
        .safeParse(await probe.json().catch(() => null));
      if (answered.success) sessionData.sessionRowId = answered.data.sessionRowId;
    }
  } catch (err) {
    // Best effort by design — the session is minted either way and the
    // layout's own `/me` call is what surfaces an outage — but not silent:
    // a probe that fails on every sign-in is a fact the logs should hold.
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth callback: /me probe failed",
    );
  }

  const sealed = await sealSession(sessionData);
  const target = new URL(pkce.returnTo || "/console", env.AUTH_URL);

  const response = NextResponse.redirect(target);
  response.cookies.set({
    name: SESSION_COOKIE,
    value: sealed,
    ...cookieOpts(SESSION_TTL_SECONDS),
  });
  response.cookies.delete(PKCE_COOKIE);
  // ⚠ **A sign-in opens the person's default organization**, as a Vercel
  // sign-in opens the default team: with no organization remembered, `/me`
  // answers the default, and `/console` lands there. A sign-out clears this
  // cookie already; a session that ended any other way — expired, revoked
  // from another device — left it behind, naming wherever the last one stood.
  response.cookies.delete(ACTIVE_ORGANIZATION_COOKIE);

  track("onboarding.signup_completed", { userId: sessionData.userId });

  return response;
}
