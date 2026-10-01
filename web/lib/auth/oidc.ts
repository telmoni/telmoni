import { z } from "zod";

import { env } from "@/lib/env";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { logger } from "@/lib/logger";

const AuthnResultSchema = z.object({
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
const AuthorizeUrlSchema = z.object({ authorizeUrl: z.url() });

// What auth's sign-in pages offer, as the deployment is configured.
const SignInConfigSchema = z.object({
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
// asks for it.
export async function getAuthorizeUrl(
  state: string,
  opts: { signUp?: boolean; loginHint?: string; provider?: SignInProvider } = {},
): Promise<string> {
  const res = await fetchWithTimeout(
    `${env.SERVER_URL}/internal/auth/start`,
    {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({
        state,
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

// Spend the code the sign-in earned. `provider` says whose code it is: the
// external provider's, or absent, one auth's own sign-in minted.
export async function exchangeCode(
  code: string,
  provider?: SignInProvider,
): Promise<AuthnResult> {
  const res = await fetchWithTimeout(
    `${env.SERVER_URL}/internal/auth/exchange`,
    {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({ code, ...(provider ? { provider } : {}) }),
    },
  );
  if (!res.ok) throw new Error(`auth exchange failed: ${res.status}`);
  return AuthnResultSchema.parse(await res.json());
}

// What the sign-in pages show. `null` when auth cannot say, which the pages
// take as the form alone: the action behind it answers for auth being down.
export async function getSignInConfig(): Promise<SignInConfig | null> {
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/auth/config`, {
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
    await fetchWithTimeout(`${env.SERVER_URL}/internal/auth/logout`, {
      method: "POST",
      headers: serviceHeaders(),
      body: JSON.stringify({ session_id: sessionId }),
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
const LogoutUrlSchema = z.object({ logoutUrl: z.url() });

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
      `${env.SERVER_URL}/internal/auth/logout-url`,
      {
        method: "POST",
        headers: serviceHeaders(),
        body: JSON.stringify({ session_id: sessionId, id_token: idToken, return_to: returnTo }),
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

// `session_row_id` names the `auth.sessions` row this sign-in was recorded
// under, so auth marks it seen as it rotates the tokens — there is no separate
// touch. A refused grant, a revoked row included, is a 401 and answers `null`,
// which ends the session.
export async function refreshTokens(
  refreshToken: string,
  sessionRowId: string | null,
): Promise<AuthnResult | null> {
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/auth/refresh`,
      {
        method: "POST",
        headers: serviceHeaders(),
        body: JSON.stringify({ refresh_token: refreshToken, session_row_id: sessionRowId }),
      },
    );
    if (!res.ok) return null;
    const parsed = AuthnResultSchema.safeParse(await res.json());
    return parsed.success ? parsed.data : null;
  } catch (err) {
    // `null` here ends the session: `getSession` deletes the cookie when a
    // refresh fails. An outage at auth during the refresh window therefore
    // signs people out, and without this line it did so with nothing in the
    // logs to say why.
    logger.warn(
      { error: err instanceof Error ? err.message : String(err) },
      "auth: token refresh failed",
    );
    return null;
  }
}
