import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { InviteScope, Role, asRole } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { type RosterCeiling, RosterCeilingSchema } from "./organization-member";
import { readJson, valueOf } from "./read";

const MemberSchema = z.object({
  id: z.string(),
  member_id: z.string(),
  email: z.string(),
  display_name: z.string().nullable().optional(),
  role: z.string(),
  created_at: z.string(),
  is_owner: z.boolean().optional(),
  /// When the owner's live offer of the project to this admin lapses; absent
  /// when there is no live offer, and always for the owner's row.
  transfer_offer_expires_at: z.string().nullable().optional(),
});

export type Member = Omit<z.infer<typeof MemberSchema>, "role"> & {
  role: Role | null;
  is_owner: boolean;
};

export type MemberAccess =
  | { kind: "ok"; members: Member[]; ceiling: RosterCeiling | null }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchMembers = cache(async (projectId: string): Promise<MemberAccess> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchMembers",
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}/members`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    z.object({
      members: z.array(MemberSchema),
      // The owning organization's, which a seat here counts against.
      ceiling: RosterCeilingSchema.nullable().optional(),
    }),
  );
  if (read.kind !== "ok") return read;
  return {
    kind: "ok",
    members: read.value.members.map((m) => {
      const role = asRole(m.role);
      return {
        ...m,
        role,
        is_owner: m.is_owner ?? (role === Role.Owner || m.role === "owner"),
      };
    }),
    ceiling: read.value.ceiling ?? null,
  };
});

const InviteSchema = z.object({
  id: z.string(),
  email: z.string(),
  role: z.string(),
  expires_at: z.string(),
  created_at: z.string(),
});

export type Invite = Omit<z.infer<typeof InviteSchema>, "role"> & {
  role: Role | null;
};

export type InviteAccess =
  | { kind: "ok"; invites: Invite[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchInvites = cache(async (projectId: string): Promise<InviteAccess> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchInvites",
    `${env.SERVER_URL}/internal/v1/projects/${encodeURIComponent(projectId)}/invites`,
    { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    z.object({ invites: z.array(InviteSchema) }),
  );
  if (read.kind !== "ok") return read;
  return {
    kind: "ok",
    invites: read.value.invites.map((i) => ({ ...i, role: asRole(i.role) })),
  };
});

const InviteLookSchema = z.object({
  /// Which roster the link seats somebody on. The two ladders spell their
  /// roles the same, so the role alone cannot say.
  scope: z.enum(InviteScope),
  inviter: z.string(),
  /// What the organization is called: its name, which it has before it can
  /// invite.
  organization: z.string(),
  email: z.string(),
  role: z.string(),
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
