import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { Role, asRole } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

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
  | { kind: "ok"; members: Member[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchMembers = cache(async (projectId: string): Promise<MemberAccess> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/projects/${encodeURIComponent(projectId)}/members`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const parsed = z
      .object({ members: z.array(MemberSchema) })
      .safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchMembers", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      members: parsed.data.members.map((m) => {
        const role = asRole(m.role);
        return {
          ...m,
          role,
          is_owner: m.is_owner ?? (role === Role.Owner || m.role === "owner"),
        };
      }),
    };
  } catch (err) {
    logger.warn(
      {
        fetcher: "fetchMembers",
        error: err instanceof Error ? err.message : String(err),
      },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
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
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/projects/${encodeURIComponent(projectId)}/invites`,
      { headers: projectHeaders(ctx, projectId), cache: "no-store" },
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) return { kind: "unavailable" };
    const parsed = z
      .object({ invites: z.array(InviteSchema) })
      .safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchInvites", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      invites: parsed.data.invites.map((i) => ({ ...i, role: asRole(i.role) })),
    };
  } catch (err) {
    logger.warn(
      {
        fetcher: "fetchInvites",
        error: err instanceof Error ? err.message : String(err),
      },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
});

const InviteLookSchema = z.object({
  /// Which roster the link seats somebody on. The two ladders spell their
  /// roles the same, so the role alone cannot say.
  scope: z.enum(["project", "organization"]),
  inviter: z.string(),
  /// What the organization is called: its name, else its owner's address.
  organization: z.string(),
  email: z.string(),
  role: z.string(),
});

export type InviteLook = z.infer<typeof InviteLookSchema>;

export async function lookUpInvite(token: string): Promise<InviteLook | null> {
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/invites/look`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-service-secret": env.SERVICE_SECRET,
        },
        body: JSON.stringify({ token }),
        cache: "no-store",
      },
    );
    if (!res.ok) return null;
    const parsed = InviteLookSchema.safeParse(await res.json());
    return parsed.success ? parsed.data : null;
  } catch (err) {
    logger.warn(
      {
        fetcher: "lookUpInvite",
        error: err instanceof Error ? err.message : String(err),
      },
      "entities: fetch failed",
    );
    return null;
  }
}
