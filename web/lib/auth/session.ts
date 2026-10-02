import "server-only";

import { sealData, unsealData } from "iron-session";
import { cookies } from "next/headers";
import { z } from "zod";

import { env } from "@/lib/env";
import { getLogoutUrl, refreshTokens, revokeSession } from "@/lib/auth/oidc";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { ACTIVE_ORGANIZATION_COOKIE } from "@/lib/proxy/organization";

export interface SessionData {
  userId: string;
  email: string;
  firstName: string | null;
  lastName: string | null;
  // Opaque: auth resolves it, and nothing here reads anything out of it.
  accessToken: string;
  refreshToken: string | null;
  // The session the tokens belong to, as the exchange answered it: what the
  // sign-out names at auth.
  sessionId: string | null;
  sessionRowId: string | null;
  expiresAt: number;
  idToken: string | null;
  authMethod: string | null;
  needsReseal?: boolean;
}

export const SESSION_COOKIE = "telmoni_session";
export const PKCE_COOKIE = "telmoni_pkce";
// One connector handshake in flight: which project it is for, which vendor, and
// the state the vendor will echo. Sealed like the PKCE cookie and for the same
// reason — the callback has to know the project before it can address a request
// to the service, and the query string is the vendor's to fill, not ours.
export const CONNECT_COOKIE = "telmoni_connect";

const REFRESH_THRESHOLD_MS = 60_000;

// How long the sealed cookie lives. Exported because the two Route Handlers
// that reseal onto a NextResponse cannot use `writeSessionCookie` below — it
// writes through `cookies()`, which a response-scoped cookie jar is not — and
// both spelled the literal out instead. Two spellings of one TTL is one that
// gets changed alone.
export const SESSION_TTL_SECONDS = 60 * 60 * 24 * 30;

const pw = () => env.AUTH_SECRET;

export async function getSession(): Promise<SessionData | null> {
  const jar = await cookies();
  const value = jar.get(SESSION_COOKIE)?.value;
  if (!value) return null;

  let data: SessionData;
  try {
    const unsealed = await unsealData<Partial<SessionData>>(value, {
      password: pw(),
    });
    if (!unsealed?.userId) return null;
    data = normalize(unsealed);
  } catch {
    return null;
  }

  if (await isSessionBlacklisted(sessionEndKey(data))) {
    try {
      jar.delete(SESSION_COOKIE);
    } catch {
    }
    return null;
  }

  if (Date.now() + REFRESH_THRESHOLD_MS < data.expiresAt) {
    return data;
  }

  const refreshed = await refreshSessionTokens(data);
  if (!refreshed) {
    try {
      jar.delete(SESSION_COOKIE);
    } catch {
    }
    return null;
  }

  try {
    const sealed = await sealSession(refreshed);
    jar.set({
      name: SESSION_COOKIE,
      value: sealed,
      ...cookieOpts(SESSION_TTL_SECONDS),
    });
  } catch {
    refreshed.needsReseal = true;
  }
  return refreshed;
}

/// Whether the session cookie names a row the console has already ended — an
/// account deletion, a confirmed email change, or this device revoked from
/// the sessions list — as opposed to no session, or one that ran out. Read it
/// before `getSession`, which deletes a blacklisted cookie as it refuses it.
export async function sessionEndedAlready(): Promise<boolean> {
  const jar = await cookies();
  const sealed = jar.get(SESSION_COOKIE)?.value;
  if (!sealed) return false;
  const data = await unsealData<{ sessionRowId?: string; sessionId?: string }>(sealed, {
    password: pw(),
  }).catch(() => null);
  if (!data) return false;
  return isSessionBlacklisted(
    sessionEndKey({ sessionRowId: data.sessionRowId ?? null, sessionId: data.sessionId ?? null }),
  );
}

export async function destroySession(
  opts: { redirectThroughProvider?: boolean } = {},
): Promise<string | null> {
  const jar = await cookies();
  const sealed = jar.get(SESSION_COOKIE)?.value;
  let providerLogoutUrl: string | null = null;
  if (sealed) {
    const data = await unsealData<{
      sessionId?: string;
      sessionRowId?: string;
      idToken?: string | null;
    }>(sealed, {
      password: pw(),
    }).catch(() => null);
    // ⚠ **A session the console has already ended never goes through the
    // provider's page.** A row is on the blacklist before sign-out after an
    // account deletion, a confirmed email change, or this very device being
    // revoked from the sessions list, and auth ended the session as it did
    // each of those. A provider's logout page is for a live session, and
    // sent an ended one it did not bring the browser back to `return_to`:
    // the farewell after an account deletion ended on the provider's page
    // instead of the home page. Ended already, the sign-out is local and the
    // route lands on `/` itself.
    const endKey = data
      ? sessionEndKey({
          sessionRowId: data.sessionRowId ?? null,
          sessionId: data.sessionId ?? null,
        })
      : null;
    const endedAlready = await isSessionBlacklisted(endKey);
    await blacklistSession(endKey);
    const sid = data?.sessionId ?? null;
    if (sid) {
      if (opts.redirectThroughProvider && !endedAlready) {
        // The external provider's page, for a session that began there;
        // auth answers the home page itself for one that began with a
        // password.
        providerLogoutUrl = await getLogoutUrl(
          sid,
          data?.idToken ?? null,
          new URL("/", env.AUTH_URL).toString(),
        );
      }
      // ⚠ **Always, whichever way the browser leaves.** This used to run only
      // when there was no provider logout URL — so every normal sign-out went
      // through the provider's page, never reached auth, and left the session
      // row live: the outstanding access token kept working until it expired.
      await revokeSession(sid);
    }
  }
  for (const name of [SESSION_COOKIE, PKCE_COOKIE, ACTIVE_ORGANIZATION_COOKIE]) {
    jar.set(name, "", { path: "/", maxAge: 0 });
  }
  return providerLogoutUrl;
}

export function cookieOpts(maxAge: number) {
  return {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax" as const,
    maxAge,
    path: "/",
  };
}

export async function sealSession(data: SessionData): Promise<string> {
  return sealData(data, { password: pw(), ttl: SESSION_TTL_SECONDS });
}

// Write `data` back into the sealed cookie.
//
// Only callable where cookies are writable — a Server Action or a Route
// Handler. A Server Component holds a ReadonlyRequestCookies and this throws,
// which is why `getSession` above wraps the same two lines in its own try/catch
// and sets `needsReseal` instead.
//
// ⚠ Exists so the cookie's NAME, FLAGS and TTL have one spelling for everything
// that writes through the request's cookie jar. The two Route Handlers that
// reseal onto a NextResponse still set the three by hand — a response-scoped
// jar is a different object — but they now spend the exported
// SESSION_TTL_SECONDS rather than re-typing the literal.
export async function writeSessionCookie(data: SessionData): Promise<void> {
  const jar = await cookies();
  jar.set({
    name: SESSION_COOKIE,
    value: await sealSession(data),
    ...cookieOpts(SESSION_TTL_SECONDS),
  });
}

// The pending sign-in `/auth/login` seals before the browser goes to prove
// who it is: the CSRF state the callback checks, where to land, and, once
// the sign-in page's "Continue with…" sent the browser to the external
// provider, whose code the callback should expect.
export interface PkceData {
  state: string;
  returnTo: string;
  provider?: "external";
}

export async function sealPkce(data: PkceData): Promise<string> {
  return sealData(data, { password: pw(), ttl: 600 });
}

const PkceData = z.object({
  state: z.string().min(1),
  returnTo: z.string().default("/console"),
  provider: z.literal("external").optional(),
});

export async function unsealPkce(value: string): Promise<PkceData | null> {
  if (!value) return null;
  try {
    const data = await unsealData<unknown>(value, { password: pw() });
    const parsed = PkceData.safeParse(data);
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}

const ConnectData = z.object({
  state: z.string().min(1),
  organizationId: z.string().min(1),
  projectId: z.string().min(1),
  provider: z.enum(["slack", "discord"]),
});

export type ConnectData = z.infer<typeof ConnectData>;

export async function sealConnect(data: ConnectData): Promise<string> {
  return sealData(data, { password: pw(), ttl: 600 });
}

export async function unsealConnect(value: string): Promise<ConnectData | null> {
  if (!value) return null;
  try {
    const data = await unsealData<unknown>(value, { password: pw() });
    const parsed = ConnectData.safeParse(data);
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}

function normalize(partial: Partial<SessionData>): SessionData {
  return {
    userId: partial.userId ?? "",
    email: partial.email ?? "",
    firstName: partial.firstName ?? null,
    lastName: partial.lastName ?? null,
    accessToken: partial.accessToken ?? "",
    refreshToken: partial.refreshToken ?? null,
    sessionId: partial.sessionId ?? null,
    sessionRowId: partial.sessionRowId ?? null,
    expiresAt: partial.expiresAt ?? 0,
    idToken: partial.idToken ?? null,
    authMethod: partial.authMethod ?? null,
  };
}

// The refresh carries the session row's id so auth marks it seen as it rotates
// the tokens; the console makes no call of its own for that. `null` ends the
// session at every caller, a revoked row included — auth refuses the grant.
export async function refreshSessionTokens(
  data: SessionData,
): Promise<SessionData | null> {
  if (!data.refreshToken) return null;
  const result = await refreshTokens(data.refreshToken, data.sessionRowId);
  if (!result) return null;
  return {
    ...data,
    accessToken: result.accessToken,
    refreshToken: result.refreshToken ?? data.refreshToken,
    sessionId: result.sessionId,
    expiresAt: Date.now() + result.expiresIn * 1000,
    idToken: data.idToken,
  };
}
