import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { OrganizationRole } from "@/lib/types/enums";

import { organizationHeaders, identityContext } from "./identity-context";
import { readJson } from "./read";

const OrganizationMemberSchema = z.object({
  id: z.string(),
  member_id: z.string(),
  email: z.string(),
  display_name: z.string().nullable().optional(),
  role: z.enum(OrganizationRole),
  created_at: z.string(),
  is_owner: z.boolean(),
  /// When the owner's live offer of the organization to this admin lapses.
  ownership_offer_expires_at: z.string().nullable().optional(),
});

export type OrganizationMember = z.infer<typeof OrganizationMemberSchema>;

// How many people the organization may hold, as the deployment that sells
// plans answers it, and what stands against that — its members and everyone
// invited — with the sentence an invitation past it is refused with; `null`
// where none is held, as on a self-hosted installation.
export const RosterCeilingSchema = z.object({
  limit: z.number().int().min(0),
  held: z.number().int().min(0),
  detail: z.string(),
});
export type RosterCeiling = z.infer<typeof RosterCeilingSchema>;

export type OrganizationMemberAccess =
  | { kind: "ok"; members: OrganizationMember[]; ceiling: RosterCeiling | null }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchOrganizationMembers = cache(
  async (): Promise<OrganizationMemberAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    const read = await readJson(
      "fetchOrganizationMembers",
      `${env.SERVER_URL}/internal/v1/organization/members`,
      { headers: organizationHeaders(ctx), cache: "no-store" },
      z.object({
        members: z.array(OrganizationMemberSchema),
        ceiling: RosterCeilingSchema.nullable().optional(),
      }),
    );
    if (read.kind !== "ok") return read;
    return { kind: "ok", members: read.value.members, ceiling: read.value.ceiling ?? null };
  },
);

const OrganizationInviteSchema = z.object({
  id: z.string(),
  email: z.string(),
  role: z.enum(OrganizationRole),
  expires_at: z.string(),
  created_at: z.string(),
});

export type OrganizationInvite = z.infer<typeof OrganizationInviteSchema>;

export type OrganizationInviteAccess =
  | { kind: "ok"; invites: OrganizationInvite[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchOrganizationInvites = cache(
  async (): Promise<OrganizationInviteAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    const read = await readJson(
      "fetchOrganizationInvites",
      `${env.SERVER_URL}/internal/v1/organization/invites`,
      { headers: organizationHeaders(ctx), cache: "no-store" },
      z.object({ invites: z.array(OrganizationInviteSchema) }),
    );
    if (read.kind !== "ok") return read;
    return { kind: "ok", invites: read.value.invites };
  },
);
