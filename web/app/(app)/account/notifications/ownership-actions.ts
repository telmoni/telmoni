"use server";

import { revalidatePath } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { isOrganizationId, isProjectId } from "@/lib/ids";
import { organizationChannel, publishEvent, publishToAll } from "@/lib/events/publisher";
import { admit } from "@/lib/server/admit";
import { personHeaders, sessionHeaders } from "@/lib/server/entities/identity-context";
import { projectPath } from "@/lib/slug";

// Answering an ownership offer. The organization is the OFFER's, named
// explicitly, not the one the console is standing in: an offer is answered
// from the notifications page, wherever the person happens to be. Auth checks
// that they hold a live offer there; this sends nothing it could trust.

async function answerOffer(
  organizationId: string,
  verb: "accept" | "decline",
): Promise<ActionResult> {
  const gate = await admit({ call: "organization:ownership-answer", limit: 10 });
  if (!gate.ok) return refused(gate.error);
  // The id travels as a header; anything but an organization id is refused
  // here rather than making `fetch` throw on a stray character.
  if (!isOrganizationId(organizationId)) return refused("That is not an organization.");

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/organization/owner-transfer/${verb}`,
    { method: "POST", headers: sessionHeaders(gate.session, organizationId) },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);

  // Everyone in the organization sees the roles move — the previous owner
  // included — and the offer leave the owner's roster.
  await publishEvent(organizationChannel(organizationId), {
    type: "ownership:changed",
    data: { organizationId },
  });
  revalidatePath("/", "layout");
  return { ok: true };
}

export async function acceptOwnershipAction(
  organizationId: string,
): Promise<ActionResult> {
  return answerOffer(organizationId, "accept");
}

export async function declineOwnershipAction(
  organizationId: string,
): Promise<ActionResult> {
  return answerOffer(organizationId, "decline");
}

// Each field read alone: one auth spells otherwise leaves the rest.
const text = z.string().optional().catch(undefined);

/** Where an answered offer left the project: its new organization and slug,
 *  the one it left, and the owner whose offer it was. */
const ProjectMoved = z.object({
  slug: text,
  organizationId: text,
  organizationSlug: text,
  previousOrganizationId: text,
  ownerOrganizationId: text,
});

/// What answering a project offer comes back with. An accept that landed
/// answers with the project's page instead, in the caller's organization now.
export type ProjectOfferAnswer = ActionResult;

// Answering the offer of a project. The project is the OFFER's, named
// explicitly, and the lane is the person's: the offer is answered from the
// notifications page, wherever the console happens to be standing. Where the
// project lands is the caller's to say among the organizations they own, and
// auth's to check.
async function answerProjectOffer(
  projectId: string,
  verb: "accept" | "decline",
  organizationId?: string,
): Promise<ProjectOfferAnswer> {
  const gate = await admit({ call: "project:offer-answer", limit: 10 });
  if (!gate.ok) return refused(gate.error);
  const { session } = gate;
  // Both ids reach auth in the URL or the body; anything but the shape each
  // has is refused here rather than making `fetch` throw on a stray character.
  if (!isProjectId(projectId)) return refused("That is not a project.");
  if (organizationId !== undefined && !isOrganizationId(organizationId)) {
    return refused("That is not an organization.");
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}/transfer/${verb}`,
    {
      method: "POST",
      headers: { ...personHeaders(session), "content-type": "application/json" },
      body: JSON.stringify(verb === "accept" && organizationId ? { organizationId } : {}),
    },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);

  const body = await readBody(res, ProjectMoved);

  if (verb === "decline") {
    const owner = body?.ownerOrganizationId;
    if (owner) {
      await publishEvent(organizationChannel(owner), {
        type: "ownership:changed",
        data: { organizationId: owner, projectId },
      });
    }
    revalidatePath("/", "layout");
    return { ok: true };
  }

  // Both organizations see the project move — the one it left, whose owner
  // is now an admin on it, and the one it landed in. The project is in the
  // destination now, under the slug it landed with: the event carries both
  // slugs, so a tab showing the project can follow it there, by slug as every
  // link is.
  const destination = body?.organizationId;
  const source = body?.previousOrganizationId;
  const organization = body?.organizationSlug;
  const project = body?.slug;
  await publishToAll(
    [
      destination ? organizationChannel(destination) : null,
      source ? organizationChannel(source) : null,
    ],
    {
      type: "ownership:changed",
      data: {
        organizationId: destination ?? source ?? "",
        projectId,
        ...(organization && project ? { organizationSlug: organization, projectSlug: project } : {}),
      },
    },
  );
  // On to the project, drawn whole with this answer: nothing is revalidated
  // first, which would have the router read it all again as soon as it
  // landed. Where auth did not say where it landed, this page is drawn again
  // without the offer.
  if (organization && project) redirect(projectPath(organization, project));
  revalidatePath("/", "layout");
  return { ok: true };
}

export async function acceptProjectOfferAction(
  projectId: string,
  organizationId?: string,
): Promise<ProjectOfferAnswer> {
  return answerProjectOffer(projectId, "accept", organizationId);
}

export async function declineProjectOfferAction(
  projectId: string,
): Promise<ProjectOfferAnswer> {
  return answerProjectOffer(projectId, "decline");
}
