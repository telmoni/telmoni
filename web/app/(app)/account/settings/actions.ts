"use server";

import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { blacklistSession, isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { env } from "@/lib/env";
import { accountHeaders, personHeaders } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";
import { canChangeEmail } from "@/lib/sign-in-method";

// Shape only, and loose on purpose. A regex that refuses a legal address is a
// bug the person cannot work around; one that admits an illegal address costs a
// round trip. Auth validates properly and the column has its own CHECK.
const emailShape = z.object({ email: z.email().max(254) });

// ⚠ The sign-in method never reaches auth — it describes the sign-in that
// sealed the cookie, and no row stores it — so the console is the ONLY place
// this rule can be enforced. A Server Action is a public endpoint, so the check
// has to live in the action and not only in the component: without it a
// provider account can invoke this directly, and move an address the provider
// would put back at its next sign-in.
const NOT_ELIGIBLE =
  "The address on this account is held by your identity provider, so it can't be changed here.";

const UNRESOLVED = "Couldn't resolve your account right now. Try again in a moment.";

export async function requestPasswordResetAction(): Promise<{
  error: string | null;
}> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return { error: "Your session expired — sign in again." };
  }

  const limited = await rateLimit(sessionKey(session, "password:reset"), {
    limit: 3,
    windowMs: 60 * 60_000,
  });
  if (limited) {
    return { error: "Too many requests — try again in an hour." };
  }

  const headers = await accountHeaders();
  if (!headers) return { error: UNRESOLVED };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me/password-reset`, {
    method: "POST",
    headers,
  });
  if (!res) return { error: "The organization service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  return { error: null };
}

// Step one: ask auth to open a pending change. The identity provider mails a
// code to the new address, and auth mails its own to the current one.
//
// Answers with the address auth normalised, so the next step can name the
// destination and post the address the codes were actually minted for.
export async function requestEmailChangeAction(
  email: string,
): Promise<{ error: string | null; email?: string }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return { error: "Your session expired — sign in again." };
  }
  if (!canChangeEmail(session.authMethod)) return { error: NOT_ELIGIBLE };

  const next = email.trim().toLowerCase();
  if (!emailShape.safeParse({ email: next }).success) {
    return { error: "Enter a valid email address." };
  }
  if (next === session.email.trim().toLowerCase()) {
    return { error: "That is already the address on this account." };
  }

  // ⚠ **Tighter in effect than its siblings, and it must stay that way.** The
  // password-reset and deletion-code lanes mail the address on the account, so
  // abusing them spams only yourself. This one makes the identity provider mail
  // an address the CALLER typed — a loose budget here is an open relay with our
  // sender reputation on it. Do not raise it to make "Resend code" feel
  // comfortable.
  const limited = await rateLimit(sessionKey(session, "email:change-code"), {
    limit: 3,
    windowMs: 60 * 60_000,
  });
  if (limited) return { error: "Too many requests — try again in an hour." };

  const headers = await accountHeaders();
  if (!headers) return { error: UNRESOLVED };

  // The new address travels in the body precisely because it has not earned
  // anything yet; the address auth mails its own code to is the one it holds.
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me/email-change`, {
    method: "POST",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify({ newEmail: next }),
  });
  if (!res) return { error: "The organization service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  return { error: null, email: next };
}

// Step two: spend both codes. On success the address has moved at the identity
// provider AND in auth's own row, and every session including this one was
// revoked server-side.
export async function confirmEmailChangeAction(
  currentCode: string,
  newCode: string,
): Promise<{ error: string | null; email?: string }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return { error: "Your session expired — sign in again." };
  }
  if (!canChangeEmail(session.authMethod)) return { error: NOT_ELIGIBLE };

  if (!/^\d{6}$/.test(currentCode) || !/^\d{6}$/.test(newCode)) {
    return { error: "Enter both 6-digit codes from your email." };
  }

  const limited = await rateLimit(sessionKey(session, "email:change-confirm"), {
    limit: 10,
    windowMs: 60 * 60_000,
  });
  if (limited) return { error: "Too many attempts. Try again in an hour." };

  const headers = await accountHeaders();
  if (!headers) return { error: UNRESOLVED };

  // The organization it is sent with is where auth records the change, and
  // auth checks the person is in it before the identity provider is asked.
  // Somebody in no organization sends none, and auth logs the change instead.
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/me/email-change/confirm`,
    {
      method: "POST",
      headers: { ...headers, "content-type": "application/json" },
      body: JSON.stringify({ currentCode, newCode }),
    },
  );
  if (!res) return { error: "The organization service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }

  // ⚠ **The session is already over; this ends the console's half of it.**
  // Auth revoked this browser's row with the change, and its person gate
  // refuses the bearer from here on, so every render after this would be pages
  // asking services that no longer answer them. The blacklist makes the next
  // navigation a sign-in instead. It is a Redis write rather than a cookie
  // write so that nothing re-renders now — that render would redirect to
  // sign-out before the person had read which address the account has — and
  // nothing is revalidated for the same reason.
  await blacklistSession(sessionEndKey(session));

  // Auth answers with the address the identity provider reported, which is
  // authoritative. Unreadable, the page names the address the codes were sent
  // to instead.
  const body = z
    .object({ email: z.email() })
    .safeParse(await res.json().catch(() => null));
  return body.success ? { error: null, email: body.data.email } : { error: null };
}

// The organization a sign-in opens in, as Vercel's default team is. The
// body names it, so no organization header goes with it: auth refuses one the
// person holds no seat in, or one being deleted, and records the choice on the
// chosen organization's chain.
export async function setDefaultOrganizationAction(
  organizationId: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) {
    return { error: "Your session expired — sign in again." };
  }

  const limited = await rateLimit(sessionKey(session, "account:default-organization"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/me/default-organization`, {
    method: "PUT",
    headers: { ...personHeaders(session), "content-type": "application/json" },
    body: JSON.stringify({ organizationId }),
  });
  if (!res) return { error: "The organization service is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { error: message };
  }
  return { error: null };
}
