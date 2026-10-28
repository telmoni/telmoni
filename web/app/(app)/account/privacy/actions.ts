"use server";

import { refresh } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { blacklistSession } from "@/lib/auth/session-blacklist";
import { NOT_A_CODE, isConfirmationCode } from "@/lib/confirmation-code";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { env } from "@/lib/env";
import { admit } from "@/lib/server/admit";
import { accountHeaders, personHeaders } from "@/lib/server/entities/identity-context";
import { AccountDeletedSchema } from "@/lib/server/entities/organization";

// The person's own lanes: they act on the account, whichever organization the
// console is standing in. The ones that change something send that
// organization — auth records them on its audit chain, after checking the
// person is in it — or, for somebody in no organization at all, none, and auth
// logs them instead. Deleting the account sends none: it must reach them from
// the account screen a closed gate leaves them.

const UNRESOLVED = "Couldn't resolve your account right now. Try again in a moment.";
const UNREACHABLE = "The organization service is unreachable. Try again.";

export async function requestAccountDeletionCodeAction(): Promise<ActionResult> {
  const gate = await admit({ call: "auth:delete-code", limit: 3, per: "hour" });
  if (!gate.ok) return refused(gate.error);

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/me/deletion-code`, {
    method: "POST",
    headers: personHeaders(gate.session),
  });
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}

/// Delete the account. Auth refuses (409, naming them) while the person owns
/// an organization anybody else is in; the organizations they own alone go
/// with them.
export async function deleteAccountAction(
  code: string,
): Promise<ActionResult<{ deleted: boolean }>> {
  if (!isConfirmationCode(code)) return refused(NOT_A_CODE);
  const gate = await admit({ call: "auth:delete-confirm", limit: 10, per: "hour" });
  if (!gate.ok) return refused(gate.error);
  const { session } = gate;

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/me`, {
    method: "DELETE",
    headers: { ...personHeaders(session), "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);

  const deleted = (await readBody(res, AccountDeletedSchema))?.deleted === true;

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
): Promise<ActionResult> {
  const gate = await admit({ call: "sessions:revoke", limit: 10 });
  if (!gate.ok) return refused(gate.error);

  const headers = await accountHeaders();
  if (!headers) return refused(UNRESOLVED);

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/auth/sessions/${encodeURIComponent(
      sessionId,
    )}/revoke`,
    {
      method: "POST",
      headers,
    },
  );
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  await blacklistSession(sessionId);
  refresh();
  return { ok: true };
}

export async function setAnalyticsPreferenceAction(
  optIn: boolean,
): Promise<ActionResult> {
  const gate = await admit({ call: "privacy:analytics", limit: 20 });
  if (!gate.ok) return refused(gate.error);

  const headers = await accountHeaders();
  if (!headers) return refused(UNRESOLVED);

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/me/analytics`, {
    method: "PUT",
    headers: {
      ...headers,
      "content-type": "application/json",
    },
    body: JSON.stringify({ optIn }),
  });
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  refresh();
  return { ok: true };
}
