"use server";

import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { env } from "@/lib/env";
import { accountHeaders, personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

// The person's own lanes: they act on the account, whichever organization the
// console is standing in. The ones that change something send that
// organization — auth records them on its audit chain, after checking the
// person is in it — or, for somebody in no organization at all, none, and auth
// logs them instead. Deleting the account sends none: it must reach them from
// the account screen a closed gate leaves them.

const UNRESOLVED = "Couldn't resolve your account right now. Try again in a moment.";
const UNREACHABLE = "The organization service is unreachable. Try again.";

export async function requestAccountDeletionCodeAction(): Promise<{
  error?: string;
  ok?: boolean;
}> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "auth:delete-code"), {
    limit: 3,
    windowMs: 60 * 60 * 1000,
  });
  if (limited) return { error: "Too many requests. Try again in an hour." };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me/deletion-code`, {
    method: "POST",
    headers: personHeaders(session),
  });
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { ok: true };
}

/// Delete the account. Auth refuses (409, naming them) while the person owns
/// an organization anybody else is in; the organizations they own alone go
/// with them.
export async function deleteAccountAction(
  code: string,
): Promise<{ error?: string; ok?: boolean; deleted?: boolean }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) redirect("/auth/login");

  if (!/^\d{6}$/.test(code)) {
    return { error: "Enter the 6-digit code from your email." };
  }

  const limited = await rateLimit(sessionKey(session, "auth:delete-confirm"), {
    limit: 10,
    windowMs: 60 * 60 * 1000,
  });
  if (limited) return { error: "Too many attempts. Try again in an hour." };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me`, {
    method: "DELETE",
    headers: { ...personHeaders(session), "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };

  const deleted = await res
    .json()
    .then((b: unknown) => (b as { deleted?: unknown } | null)?.deleted === true)
    .catch(() => false);

  // ⚠ **The session is already over; this ends the console's half of it.**
  // Auth revoked every session the person had along with the account, so any
  // render after this would be pages asking services that no longer answer
  // them; the blacklist turns the next navigation into a sign-out instead. It
  // is a Redis write and not the cookie deletion signing out does because a
  // cookie write re-renders the page in this response, and the (app) layout —
  // finding no session — would redirect to sign-out before the farewell was
  // ever on screen. The farewell's timer and button go to `/auth/logout`,
  // which finds the row blacklisted, signs out locally rather than through
  // the provider's page (auth revoked that session with the deletion, and the
  // page wants a live one), and lands on the home page.
  await blacklistSession(sessionEndKey(session));

  return { ok: true, deleted };
}

export async function revokeSessionAction(
  sessionId: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return { error: "Your session expired — sign in again." };
  }

  const limited = await rateLimit(sessionKey(session, "sessions:revoke"), {
    limit: 10,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const headers = await accountHeaders();
  if (!headers) return { error: UNRESOLVED };

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/auth/sessions/${encodeURIComponent(
      sessionId,
    )}/revoke`,
    {
      method: "POST",
      headers,
    },
  );
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  await blacklistSession(sessionId);
  return { error: null };
}

export async function setAnalyticsPreferenceAction(
  optIn: boolean,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };

  const limited = await rateLimit(sessionKey(session, "privacy:analytics"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const headers = await accountHeaders();
  if (!headers) return { error: UNRESOLVED };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me/analytics`, {
    method: "PUT",
    headers: {
      ...headers,
      "content-type": "application/json",
    },
    body: JSON.stringify({ opt_in: optIn }),
  });
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  return { error: null };
}
