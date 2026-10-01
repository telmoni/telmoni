// Pure, so that a Server Action can import it beside a mocked `session`
// module: nothing here touches a cookie jar or a secret.

/// The key the console ends a session under, and checks it under: auth's
/// session row, or — for a sign-in whose probe of `/me` never recorded one
/// (`callback/route.ts` seals `sessionRowId: null` then) — the session id the
/// exchange answered with. Both name this one device. Without the fallback, a
/// row-less session could never be marked ended: an account deletion's
/// farewell took the provider's logout page instead of the home page, and the
/// cookie was refused only once auth refused the bearer.
export function sessionEndKey(session: {
  sessionRowId: string | null;
  sessionId: string | null;
}): string | null {
  return session.sessionRowId ?? session.sessionId;
}
