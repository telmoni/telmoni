"use server";


import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { publishToAll, userChannel } from "@/lib/events/publisher";
import { admitOrganization } from "@/lib/server/admit";
import { organizationHeaders } from "@/lib/server/entities/identity-context";
import { OrganizationDeletedSchema } from "@/lib/server/entities/organization";
import { fetchOrganizationMembers } from "@/lib/server/entities/organization-member";

// Deleting an ORGANIZATION — the one the settings page rendered, which must
// still be the one the console is standing in — and nobody's account. The
// owner stays signed in and lands in whatever organization they still belong
// to. If that was their last, auth gives them a new one while sign-ups are
// open, and the console gives them their account alone while they are closed.

const UNREACHABLE = "The organization service is unreachable. Try again.";

// Presentation only: auth refuses anybody but the owner, and says so.
const OWNER_ONLY = { role: "owner", refused: "Only this organization's owner can delete it." } as const;

export async function requestOrganizationDeletionCodeAction(
  organizationId: string,
): Promise<ActionResult> {
  // The code is bound to the organization it is minted for, so minting it for
  // one the page is not showing is how typing it deletes the wrong one.
  const gate = await admitOrganization(
    { call: "organization:delete-code", limit: 3, per: "hour" },
    organizationId,
    OWNER_ONLY,
  );
  if (!gate.ok) return refused(gate.error);

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/organization/deletion-code`,
    { method: "POST", headers: organizationHeaders(gate.ctx) },
  );
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}

/// Delete the organization. Auth answers 202 once it is closed: nobody can act
/// in it from that moment, and its data is erased by the sweep once a short
/// grace has passed, with no way back.
export async function deleteOrganizationAction(
  organizationId: string,
  code: string,
): Promise<ActionResult<{ purged: boolean | null }>> {
  if (!/^\d{6}$/.test(code)) {
    return refused("Enter the 6-digit code from your email.");
  }
  const gate = await admitOrganization(
    { call: "organization:delete-confirm", limit: 10, per: "hour" },
    organizationId,
    OWNER_ONLY,
  );
  if (!gate.ok) return refused(gate.error);
  const { ctx } = gate;

  // Everybody else's open console is told, each by their own channel: the
  // organization's channel would reach the owner's tab too, and sweep the
  // success screen below away. The roster is read first, because the closed
  // organization answers nothing afterwards.
  const roster = await fetchOrganizationMembers();
  const others =
    roster.kind === "ok" ? roster.members.filter((m) => m.memberId !== ctx.userId) : [];

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/organization`, {
    method: "DELETE",
    headers: { ...organizationHeaders(ctx), "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  if (!res) return refused(UNREACHABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);

  await publishToAll(
    others.map((m) => userChannel(m.email)),
    { type: "membership:removed", data: { organizationId: ctx.organizationId } },
  );

  const answer = await readBody(res, OrganizationDeletedSchema);
  // Whether the request's purge hook landed. It is retried every ten minutes
  // when it did not, and the success screen says so rather than announcing a
  // cleanup that has not happened yet.
  const purged = answer?.purged ?? null;

  // ⚠ **Nothing is revalidated and no cookie is written, on purpose.** Either
  // makes Next re-render this page inside the action's own response, and by
  // then `/me` has moved on: the cookie names an organization that is closed,
  // so auth answers with another one, and the form still on screen would
  // announce THAT one deleted — or vanish with its danger zone. The success
  // screen's Continue is a full navigation, which re-reads everything.
  return { ok: true, purged };
}
