/// The heartbeat's refusal for a session the console has already ended — an
/// account deletion, a confirmed email change, or this device revoked from
/// the sessions list — as opposed to no session, or one that ran out.
export const SESSION_ENDED = "session_ended";

/// Where a refused heartbeat sends the browser. A session the console ended
/// goes through `/auth/logout`, which lands a session like that on the home
/// page: auth already ended it everywhere, the provider included. Any other
/// refusal is a session that ran out, and signing in again is the way back.
///
/// ⚠ **The farewell after an account deletion is why.** The heartbeat fires
/// on its own schedule, and one that landed inside the farewell's few seconds
/// sent the person to sign in, not to the home page the farewell promises.
export function afterRefusedHeartbeat(body: unknown): "/auth/logout" | "/auth/login" {
  const error = (body as { error?: unknown } | null)?.error;
  return error === SESSION_ENDED ? "/auth/logout" : "/auth/login";
}
