"use server";

import { redirect } from "next/navigation";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { isSessionBlacklisted } from "@/lib/auth/session-blacklist";
import { sessionEndKey } from "@/lib/auth/session-end-key";
import { env } from "@/lib/env";
import { publishToAll, userChannel } from "@/lib/events/publisher";
import { identityContext, organizationHeaders } from "@/lib/server/entities/identity-context";
import { fetchOrganizationMembers } from "@/lib/server/entities/organization-member";
import { SWITCHED_ORGANIZATION, unplacedOrganization } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

// Deleting an ORGANIZATION — the one the settings page rendered, which must
// still be the one the console is standing in — and nobody's account. The
// owner stays signed in and lands in whatever organization they still belong
// to. If that was their last, auth gives them a new one while sign-ups are
// open, and the console gives them their account alone while they are closed.

const UNREACHABLE = "The organization service is unreachable. Try again.";

export async function requestOrganizationDeletionCodeAction(
  organizationId: string,
): Promise<{
  error?: string;
  ok?: boolean;
}> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "organization:delete-code"), {
    limit: 3,
    windowMs: 60 * 60 * 1000,
  });
  if (limited) return { error: "Too many requests. Try again in an hour." };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  // The code is bound to the organization it is minted for, so minting it for
  // one the page is not showing is how typing it deletes the wrong one.
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  // Presentation only: auth refuses anybody but the owner, and says so.
  if (ctx.role !== "owner") {
    return { error: "Only this organization's owner can delete it." };
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/organization/deletion-code`,
    { method: "POST", headers: organizationHeaders(ctx) },
  );
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { ok: true };
}

/// Delete the organization. Auth answers 202 once it is closed: nobody can act
/// in it from that moment, and its data is erased by the sweep once a short
/// grace has passed, with no way back.
export async function deleteOrganizationAction(
  organizationId: string,
  code: string,
): Promise<{ error?: string; ok?: boolean; purged?: boolean }> {
  const session = await getServerSession();
  if (!session || (await isSessionBlacklisted(sessionEndKey(session)))) redirect("/auth/login");

  if (!/^\d{6}$/.test(code)) {
    return { error: "Enter the 6-digit code from your email." };
  }

  const limited = await rateLimit(sessionKey(session, "organization:delete-confirm"), {
    limit: 10,
    windowMs: 60 * 60 * 1000,
  });
  if (limited) return { error: "Too many attempts. Try again in an hour." };

  const ctx = await identityContext();
  if (!ctx) return { error: await unplacedOrganization() };
  if (ctx.organizationId !== organizationId) return { error: SWITCHED_ORGANIZATION };
  if (ctx.role !== "owner") {
    return { error: "Only this organization's owner can delete it." };
  }

  // Everybody else's open console is told, each by their own channel: the
  // organization's channel would reach the owner's tab too, and sweep the
  // success screen below away. The roster is read first, because the closed
  // organization answers nothing afterwards.
  const roster = await fetchOrganizationMembers();
  const others =
    roster.kind === "ok" ? roster.members.filter((m) => m.member_id !== ctx.userId) : [];

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/organization`, {
    method: "DELETE",
    headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };

  await publishToAll(
    others.map((m) => userChannel(m.email)),
    { type: "membership:removed", data: { organizationId: ctx.organizationId } },
  );

  const answer = await res
    .json()
    .then((b: unknown) => b as { purged?: unknown } | null)
    .catch(() => null);
  // Whether the request's purge hook landed. It is retried every ten minutes
  // when it did not, and the success screen says so rather than announcing a
  // cleanup that has not happened yet.
  const purged = typeof answer?.purged === "boolean" ? answer.purged : undefined;

  // ⚠ **Nothing is revalidated and no cookie is written, on purpose.** Either
  // makes Next re-render this page inside the action's own response, and by
  // then `/me` has moved on: the cookie names an organization that is closed,
  // so auth answers with another one, and the form still on screen would
  // announce THAT one deleted — or vanish with its danger zone. The success
  // screen's Continue is a full navigation, which re-reads everything.
  return { ok: true, purged };
}
