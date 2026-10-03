import { cache } from "react";
import { cookies, headers } from "next/headers";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { type FlagSet, FlagSetSchema } from "@/lib/flags";
import { logger } from "@/lib/logger";
import { ACTIVE_ORGANIZATION_COOKIE, ORGANIZATION_HEADER } from "@/lib/proxy/organization";
import {
  IncomingInviteSchema,
  type IncomingInvite,
} from "@/lib/types/incoming-invite";
import { type Role, asRole } from "@/lib/types/enums";

import { getServerSession } from "../session";

/// The person behind the session, as auth recorded them from the identity
/// provider. Theirs alone, whichever organization they are standing in.
const PersonSchema = z.object({
  userId:         z.string(),
  email:          z.string(),
  displayName:    z.string().nullable().optional(),
  analyticsOptIn: z.boolean(),
});
export type Person = z.infer<typeof PersonSchema>;

/// One organization the person is in, and as what. ⚠ An organization is
/// NOBODY: it is labelled by the name it was given (`organizationLabel`),
/// never by a person, and the person's own organization is simply the entries
/// where they are `owner`. `name` is null only before the owner has given one,
/// which the console asks for before it opens to them.
const OrganizationEntrySchema = z.object({
  organizationId:          z.string(),
  /// Where its paths begin: `/{slug}` (`lib/slug.ts`).
  slug:                    z.string(),
  name:                    z.string().nullable().optional(),
  ownerEmail:              z.string().nullable().optional(),
  ownerDisplayName:        z.string().nullable().optional(),
  role:                    z.enum(["owner", "admin", "member"]),
  /// When the owner's offer of this organization to the person lapses; absent
  /// when there is no live offer to them.
  ownershipOfferExpiresAt: z.string().nullable().optional(),
});
export type OrganizationEntry = z.infer<typeof OrganizationEntrySchema>;

/// An organization the person OWNS that is being deleted: closed to everyone,
/// its row waiting to go. `restorable` while it was the owner's own deletion
/// and the window is open; an organization Telmoni closed is listed and not
/// restorable, so the page can say so. Never the active one, never listed
/// among `organizations`.
const DeletedOrganizationSchema = z.object({
  organizationId:      z.string(),
  name:                z.string().nullable().optional(),
  deletionRequestedAt: z.string(),
  /// When the row goes: the end of the restore window.
  eraseAfter:          z.string(),
  restorable:          z.boolean(),
});
export type DeletedOrganization = z.infer<typeof DeletedOrganizationSchema>;

/// A project whose owner has offered it to the person, and whose offer is
/// still open: what is on offer, who is offering, and the organization it
/// would leave, labelled the way the console labels one (`organizationLabel`).
const ProjectOfferSchema = z.object({
  projectId:        z.string(),
  name:             z.string(),
  organizationId:   z.string(),
  organizationName: z.string().nullable().optional(),
  ownerEmail:       z.string().nullable().optional(),
  ownerDisplayName: z.string().nullable().optional(),
  expiresAt:        z.string(),
});
export type ProjectOffer = z.infer<typeof ProjectOfferSchema>;

/// A project seat the person holds, in the organization that holds it.
const MembershipSchema = z.object({
  projectId:      z.string(),
  organizationId: z.string(),
  role:           z.string(),
});
export type Membership = {
  projectId: string;
  organizationId: string;
  role: Role | null;
};

export {
  IncomingInviteSchema,
  type IncomingInvite,
} from "@/lib/types/incoming-invite";

export interface ServerContext {
  person: Person;
  organizations: OrganizationEntry[];
  /// The organizations the person owns that are being deleted, soonest to go
  /// first, for the privacy page and the account screen to offer back.
  deletedOrganizations: DeletedOrganization[];
  /// The organization this request acts in, as AUTH resolved it: the one the
  /// path names — off an organization's path, the one the cookie remembers —
  /// when the person is in it, else the oldest they own, else the oldest they
  /// belong to. The console never decides this itself.
  ///
  /// `null` for a person in no organization — sign-ups closed, or their last
  /// one gone a moment ago. They are signed in all the same, and the (app)
  /// layout gives them their invitations and their account instead of the
  /// console (`accountOnlyReason`).
  activeOrganizationId: string | null;
  /// ⚠ The path named an organization and auth answered with another: the
  /// person is not in it, or a URL change has moved its slug. Auth falls back to
  /// one of the person's own rather than refuse, which is right for a stale
  /// cookie and wrong for a path — the page would show, and its actions act
  /// in, an organization its address does not name. `activeOrganization`
  /// answers nobody then, and a page is not found.
  ///
  /// Checked on every read, not once in a layout: the router keeps a layout
  /// across a move between an organization's pages, so a check there does not
  /// run again, and a page renders beside its layout, not after it.
  organizationNotFound: boolean;
  memberships: Membership[];
  incomingInvites: IncomingInvite[];
  /// The projects offered to the person and still open to answer.
  projectOffers: ProjectOffer[];
  flags: FlagSet;
}

/// The entry for the organization the request acts in; nobody when the path
/// names one auth did not answer with (`organizationNotFound`).
export function activeOrganization(ctx: ServerContext): OrganizationEntry | null {
  if (ctx.organizationNotFound) return null;
  return ctx.organizations.find((o) => o.organizationId === ctx.activeOrganizationId) ?? null;
}

// `headers()` is request-scoped and throws outside one (a test, a build-time
// render); a missing user agent is not a reason to skip `/me`.
async function userAgent(): Promise<string | null> {
  try {
    return (await headers()).get("user-agent");
  } catch {
    return null;
  }
}

/// The organization to act in: the one the path names, by its slug, else the
/// one the cookie remembers, by its id. A request, not a claim: auth answers
/// with it only when the person is in it.
async function requestedOrganization(): Promise<{ slug: string } | { id: string } | null> {
  try {
    const named = (await headers()).get(ORGANIZATION_HEADER);
    if (named) return { slug: named };
  } catch {
    // Outside a request (a test, a build-time render) there is no path.
  }
  try {
    const remembered = (await cookies()).get(ACTIVE_ORGANIZATION_COOKIE)?.value;
    return remembered ? { id: remembered } : null;
  } catch {
    return null;
  }
}

export const getServerContext = cache(
  async (): Promise<ServerContext | null> => {
    const session = await getServerSession();
    if (!session) return null;

    try {
      // A bearer lane: the token names the person, and auth describes them
      // from what the provider told it at the exchange — it refuses an email or a
      // name here. The body carries only the browser behind this session,
      // for the sessions table; the header only which organization to act in.
      const requested = await requestedOrganization();
      const named = requested && "slug" in requested ? requested.slug : null;
      const res = await fetchWithTimeout(`${env.SERVER_URL}/me`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${session.accessToken}`,
          "content-type": "application/json",
          "x-service-secret": env.SERVICE_SECRET,
          "x-request-id": crypto.randomUUID(),
          ...(named !== null ? { "x-organization-slug": named } : {}),
          ...(requested && "id" in requested ? { "x-organization-id": requested.id } : {}),
        },
        body: JSON.stringify({ userAgent: await userAgent() }),
      });
      if (!res.ok) return null;
      const parsed = z
        .object({
          person:               PersonSchema,
          organizations:        z.array(OrganizationEntrySchema),
          deletedOrganizations: z.array(DeletedOrganizationSchema).optional(),
          activeOrganizationId: z.string().nullable(),
          memberships:          z.array(MembershipSchema).optional(),
          incomingInvites:      z.array(IncomingInviteSchema).optional(),
          projectOffers:        z.array(ProjectOfferSchema).optional(),
          flags:                FlagSetSchema.optional(),
        })
        .safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "getServerContext", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      const d = parsed.data;
      const active = d.organizations.find((o) => o.organizationId === d.activeOrganizationId);
      return {
        person:               d.person,
        organizations:        d.organizations,
        deletedOrganizations: d.deletedOrganizations ?? [],
        activeOrganizationId: d.activeOrganizationId,
        organizationNotFound: named !== null && active?.slug !== named,
        memberships:          (d.memberships ?? []).map((m) => ({ ...m, role: asRole(m.role) })),
        incomingInvites:      d.incomingInvites ?? [],
        projectOffers:        d.projectOffers ?? [],
        flags:                d.flags ?? {},
      };
    } catch {
      return null;
    }
  },
);
