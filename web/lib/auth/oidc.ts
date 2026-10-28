import { z } from "zod";

import { env } from "@/lib/env";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { Problem } from "@/lib/api/problem";
import { ExchangeRefused } from "@/lib/auth/refusal";
import { logger } from "@/lib/logger";

export const AuthnResultSchema = z.object({
  userId: z.string(),
  email: z.string().nullable(),
  emailVerified: z.boolean(),
  firstName: z.string().nullable(),
  lastName: z.string().nullable(),
  // Opaque: relayed as the bearer, never read.
  accessToken: z.string(),
  // The session the tokens belong to, which the sign-out names.
  sessionId: z.string(),
  refreshToken: z.string().nullable(),
  expiresIn: z.number(),
  // The external provider's id token, when the session began there: kept
  // for the sign-out that names it. A refresh carries none.
  idToken: z.string().nullish(),
  authMethod: z.string().nullish(),
});
export type AuthnResult = z.infer<typeof AuthnResultSchema>;

// The one way in beside the accounts auth holds itself. Named on the start
// and the exchange, and sealed into the pending sign-in in between.
export type SignInProvider = "external";

// ⚠ **`z.url()` and not `z.string()` — these two strings are spent as
// `NextResponse.redirect(...)`, which throws on anything it cannot parse.**
// `/auth/login` wraps this call in a try/catch to answer a friendly 503 when
// auth is down, but the redirect happens AFTER the catch: a malformed
// `authorizeUrl` skipped the 503 entirely and raised an unhandled TypeError on
// the sign-in path. Parseability is the bar, not an origin allowlist — the
// value is minted by our own auth service and the host is the identity
// provider's to change.
export const AuthorizeUrlSchema = z.object({ authorizeUrl: z.url() });

// What auth's sign-in pages offer, as the deployment is configured.
export const SignInConfigSchema = z.object({
  passwordSignIn: z.boolean(),
  allowSignUp: z.boolean(),
  verifyEmail: z.boolean(),
  external: z.object({ name: z.string() }).nullable(),
});
export type SignInConfig = z.infer<typeof SignInConfigSchema>;

const serviceHeaders = (): Record<string, string> => ({
  "content-type": "application/json",
  "x-service-secret": env.SERVICE_SECRET,
});

// Where the browser goes to sign in: the console's own page while the login
// form is on, the external provider's when it is off or when `provider`
// asks for it. The challenge and the nonce bind the code to this sign-in.
export async function getAuthorizeUrl(
  state: string,
  opts: {
    codeChallenge: string;
    nonce: string;
    signUp?: boolean;
    loginHint?: string;
    provider?: SignInProvider;
  },
): Promise<string> {
  const res = await fetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/auth/start`,
    {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({
        state,
        codeChallenge: opts.codeChallenge,
        nonce: opts.nonce,
        ...(opts.signUp ? { signUp: true } : {}),
        ...(opts.loginHint ? { loginHint: opts.loginHint } : {}),
        ...(opts.provider ? { provider: opts.provider } : {}),
      }),
    },
  );
  if (!res.ok) throw new Error(`auth start failed: ${res.status}`);
  const parsed = AuthorizeUrlSchema.safeParse(await res.json());
  if (!parsed.success) throw new Error("auth start: malformed response");
  return parsed.data.authorizeUrl;
}

// Spend the code the sign-in earned, with the verifier and the nonce its
// cookie sealed. `provider` says whose code it is: the external provider's,
// or absent, one auth's own sign-in minted.
export async function exchangeCode(
  code: string,
  proof: { codeVerifier: string; nonce: string },
  provider?: SignInProvider,
): Promise<AuthnResult> {
  const res = await fetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/auth/exchange`,
    {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({
        code,
        codeVerifier: proof.codeVerifier,
        nonce: proof.nonce,
        ...(provider ? { provider } : {}),
      }),
    },
  );
  if (!res.ok) {
    // The problem's type is what tells the person why, on `/auth/refused`.
    const problem = Problem.safeParse(await res.json().catch(() => null));
    throw new ExchangeRefused(res.status, problem.success ? (problem.data.type ?? null) : null);
  }
  return AuthnResultSchema.parse(await res.json());
}

// What the sign-in pages show. `null` when auth cannot say, which the pages
// take as the form alone: the action behind it answers for auth being down.
export async function getSignInConfig(): Promise<SignInConfig | null> {
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/auth/config`, {
      method: "GET",
      headers: serviceHeaders(),
    });
    if (!res.ok) return null;
    const parsed = SignInConfigSchema.safeParse(await res.json());
    return parsed.success ? parsed.data : null;
  } catch (err) {
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth: sign-in configuration unavailable",
    );
    return null;
  }
}

// Ends the session at auth: its row, which is what refuses the outstanding
// access token, and its refresh tokens.
export async function revokeSession(sessionId: string): Promise<void> {
  try {
    await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/auth/logout`, {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({ sessionId }),
    });
  } catch (err) {
    // Best effort — the cookie is gone either way — but a session that
    // outlives its sign-out is a fact worth a line.
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth: session revoke failed",
    );
  }
}

// Same reason as `AuthorizeUrlSchema`. This one already has a null fallback
// the caller honours, so validating here turns a 500 on sign-out into the
// plain "/" redirect that was always the intended degraded path.
export const LogoutUrlSchema = z.object({ logoutUrl: z.url() });

/** What the built-in form's sign-in and sign-up answer: a code, exchanged as
 *  a provider's would be. */
export const SignInCodeSchema = z.object({ code: z.string().min(1) });

// Where the browser finishes the sign-out. `idToken` is the external
// provider's, sealed at sign-in, for a session that began there; auth
// answers `returnTo` itself for one that began with a password.
export async function getLogoutUrl(
  sessionId: string,
  idToken: string | null,
  returnTo: string,
): Promise<string | null> {
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/auth/logout-url`,
      {
        method: "POST",
        headers: serviceHeaders(),
        body: JSON.stringify({ sessionId, idToken, returnTo }),
      },
    );
    if (!res.ok) return null;
    const parsed = LogoutUrlSchema.safeParse(await res.json());
    return parsed.success ? parsed.data.logoutUrl : null;
  } catch (err) {
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth: logout url unavailable, signing out locally",
    );
    return null;
  }
}

/// What auth answered a refresh: the next pair, its refusal, or nothing to go
/// on.
export type RefreshAnswer =
  | { kind: "granted"; tokens: AuthnResult }
  | { kind: "refused" }
  | { kind: "unreachable" };

// `sessionRowId` names the `auth.sessions` row this sign-in was recorded
// under, so auth marks it seen as it rotates the tokens — there is no separate
// touch. A refusal — the token spent, lapsed or revoked, the row with it — is
// a 4xx, and asking again would be refused again: the session is over. No
// answer — a timeout, a 429, a 5xx — says nothing about the session, so it is
// told apart, and the caller keeps the session and asks again later rather
// than signing somebody out because auth blinked.
export async function refreshTokens(
  refreshToken: string,
  sessionRowId: string | null,
): Promise<RefreshAnswer> {
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/auth/refresh`,
      {
        method: "POST",
        headers: serviceHeaders(),
        body: JSON.stringify({ refreshToken, sessionRowId }),
      },
    );
    if (res.ok) {
      let body: unknown;
      try {
        body = await res.json();
      } catch {
        // Cut off or garbled on its way: auth rotated, and takes the spent
        // token back for its thirty seconds, so asked again inside them it
        // answers. The parser's message is not logged: it quotes the body,
        // and the body holds the tokens.
        logger.warn("auth: token refresh answer unread");
        return { kind: "unreachable" };
      }
      const parsed = AuthnResultSchema.safeParse(body);
      // A grant read whole but of another shape reads no better asked again.
      return parsed.success ? { kind: "granted", tokens: parsed.data } : { kind: "refused" };
    }
    if (res.status >= 400 && res.status < 500 && res.status !== 408 && res.status !== 429) {
      return { kind: "refused" };
    }
    logger.warn({ status: res.status }, "auth: token refresh unanswered");
    return { kind: "unreachable" };
  } catch (err) {
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth: token refresh failed",
    );
    return { kind: "unreachable" };
  }
}
