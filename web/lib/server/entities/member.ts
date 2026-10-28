import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { InviteScope, Role } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { type RosterCeiling, RosterCeilingSchema } from "./organization-member";
import { readJson, valueOf } from "./read";

const MemberSchema = z.object({
  id: z.string(),
  member_id: z.string(),
  email: z.string(),
  display_name: z.string().nullable().optional(),
  role: z.enum(Role),
  created_at: z.string(),
  /// When the owner's live offer of the project to this admin lapses; absent
  /// when there is no live offer, and always for the owner's row.
  offer_expires_at: z.string().nullable().optional(),
});

export type Member = z.infer<typeof MemberSchema>;

export type MemberAccess =
  | { kind: "ok"; members: Member[]; ceiling: RosterCeiling | null }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const MemberListSchema = z.object({
  members: z.array(MemberSchema),
  // The owning organization's, which a seat here counts against.
  ceiling: RosterCeilingSchema.nullable().optional(),
});

export const fetchMembers = cache(async (projectId: string): Promise<MemberAccess> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchMembers",
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}/members`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    MemberListSchema,
  );
  if (read.kind !== "ok") return read;
  return { kind: "ok", members: read.value.members, ceiling: read.value.ceiling ?? null };
});

const InviteSchema = z.object({
  id: z.string(),
  email: z.string(),
  role: z.enum(Role),
  expires_at: z.string(),
  created_at: z.string(),
});

export type Invite = z.infer<typeof InviteSchema>;

export type InviteAccess =
  | { kind: "ok"; invites: Invite[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const InviteListSchema = z.object({ invites: z.array(InviteSchema) });

export const fetchInvites = cache(async (projectId: string): Promise<InviteAccess> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchInvites",
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}/invites`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    InviteListSchema,
  );
  if (read.kind !== "ok") return read;
  return { kind: "ok", invites: read.value.invites };
});

export const InviteLookSchema = z.object({
  /// Which roster the link seats somebody on. The two ladders spell their
  /// roles the same, so the role alone cannot say.
  scope: z.enum(InviteScope),
  inviter: z.string(),
  /// What the organization is called: its name, which it has before it can
  /// invite.
  organization: z.string(),
  email: z.string(),
  role: z.enum(Role),
});

export type InviteLook = z.infer<typeof InviteLookSchema>;

// A link expired or withdrawn is auth's 404: an answer, not an outage.
export async function lookUpInvite(token: string): Promise<InviteLook | null> {
  return valueOf(
    await readJson(
      "lookUpInvite",
      `${env.SERVER_URL}/internal/v1/invites/look`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-service-secret": env.SERVICE_SECRET,
        },
        body: JSON.stringify({ token }),
        cache: "no-store",
      },
      InviteLookSchema,
      { 404: "missing" },
    ),
  );
}
