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
}

export const SESSION_COOKIE = "telmoni_session";
export const PKCE_COOKIE = "telmoni_pkce";
// One connector handshake in flight: which project it is for, which vendor, and
// the state the vendor will echo. Sealed like the PKCE cookie and for the same
// reason — the callback has to know the project before it can address a request
// to the service, and the query string is the vendor's to fill, not ours.
export const CONNECT_COOKIE = "telmoni_connect";

// How long the sealed cookie lives. Exported because what reseals onto a
// NextResponse (the sign-in's callback, the heartbeat and the proxy's
// renewal) cannot use `writeSessionCookie` below, which writes
// through `cookies()`, as a response-scoped cookie jar does not, and each
// spelled the literal out instead. Two spellings of one TTL is one that gets
// changed alone.
export const SESSION_TTL_SECONDS = 60 * 60 * 24 * 30;

const pw = () => env.AUTH_SECRET;

/// The session the request carries, as the proxy left it: renewed there
/// before anything renders, once its bearer is about to lapse
/// (`lib/proxy/session.ts`).
///
/// ⚠ **Never renews it.** A page cannot write the cookie, so a refresh made
/// here was lost with the render, and auth takes a refresh token presented
/// again past its thirty seconds' grace as a copy somebody kept, and ends the
/// whole session: the home page, opened twice on a lapsed bearer, signed its
/// reader out. Only what writes the cookie it renews spends the refresh
/// token — the proxy, and the heartbeat's Route Handler.
export async function getSession(): Promise<SessionData | null> {
  const jar = await cookies();
  const data = await unsealSession(jar.get(SESSION_COOKIE)?.value);
  if (!data) return null;

  if (await isSessionBlacklisted(sessionEndKey(data))) {
    try {
      jar.delete(SESSION_COOKIE);
    } catch {
    }
    return null;
  }
  return data;
}

/// The session a sealed cookie holds; `null` for one that does not unseal or
/// names nobody.
export async function unsealSession(value: string | undefined): Promise<SessionData | null> {
  if (!value) return null;
  try {
    const unsealed = await unsealData<Partial<SessionData>>(value, {
      password: pw(),
    });
    return unsealed?.userId ? normalize(unsealed) : null;
  } catch {
    return null;
  }
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
  // The connector handshake's cookie goes too: left behind, a handshake begun
  // before the sign-out would resume for whoever signs in next on this browser.
  for (const name of [SESSION_COOKIE, PKCE_COOKIE, ACTIVE_ORGANIZATION_COOKIE, CONNECT_COOKIE]) {
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
// which is why a lapsing session is renewed in the proxy rather than by the
// page that finds it.
//
// ⚠ Exists so the cookie's NAME, FLAGS and TTL have one spelling for everything
// that writes through the request's cookie jar. What reseals onto a
// NextResponse still sets the three by hand — a response-scoped jar is a
// different object — but spends the exported SESSION_TTL_SECONDS and
// `cookieOpts` rather than re-typing them.
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
  provider: z.enum(["slack"]),
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

/// A session renewed, the renewal refused, or auth not answering it
/// (`refreshTokens`): the session stands then, and is asked about again.
export type Renewal =
  | { kind: "renewed"; session: SessionData }
  | { kind: "refused" }
  | { kind: "unreachable" };

// The refresh carries the session row's id so auth marks it seen as it rotates
// the tokens; the console makes no call of its own for that. A revoked row is
// a refused grant, and so is a session with no refresh token to spend.
export async function refreshSessionTokens(data: SessionData): Promise<Renewal> {
  if (!data.refreshToken) return { kind: "refused" };
  const answer = await refreshTokens(data.refreshToken, data.sessionRowId);
  if (answer.kind !== "granted") return answer;
  const { tokens } = answer;
  return {
    kind: "renewed",
    session: {
      ...data,
      accessToken: tokens.accessToken,
      refreshToken: tokens.refreshToken ?? data.refreshToken,
      sessionId: tokens.sessionId,
      expiresAt: Date.now() + tokens.expiresIn * 1000,
      idToken: data.idToken,
    },
  };
}
