import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { type OrganizationRole, asOrganizationRole } from "@/lib/types/organization-role";

import { organizationHeaders, identityContext } from "./identity-context";

const OrganizationMemberSchema = z.object({
  id: z.string(),
  member_id: z.string(),
  email: z.string(),
  display_name: z.string().nullable().optional(),
  role: z.string(),
  created_at: z.string(),
  is_owner: z.boolean(),
  /// When the owner's live offer of the organization to this admin lapses.
  ownership_offer_expires_at: z.string().nullable().optional(),
});

export type OrganizationMember = Omit<z.infer<typeof OrganizationMemberSchema>, "role"> & {
  role: OrganizationRole | null;
};

export type OrganizationMemberAccess =
  | { kind: "ok"; members: OrganizationMember[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchOrganizationMembers = cache(
  async (): Promise<OrganizationMemberAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/organization/members`,
        { headers: organizationHeaders(ctx), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) return { kind: "unavailable" };
      const parsed = z
        .object({ members: z.array(OrganizationMemberSchema) })
        .safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchOrganizationMembers", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return {
        kind: "ok",
        members: parsed.data.members.map((m) => ({
          ...m,
          role: asOrganizationRole(m.role),
        })),
      };
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchOrganizationMembers",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);

const OrganizationInviteSchema = z.object({
  id: z.string(),
  email: z.string(),
  role: z.string(),
  expires_at: z.string(),
  created_at: z.string(),
});

export type OrganizationInvite = Omit<z.infer<typeof OrganizationInviteSchema>, "role"> & {
  role: OrganizationRole | null;
};

export type OrganizationInviteAccess =
  | { kind: "ok"; invites: OrganizationInvite[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

export const fetchOrganizationInvites = cache(
  async (): Promise<OrganizationInviteAccess> => {
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/organization/invites`,
        { headers: organizationHeaders(ctx), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (!res.ok) return { kind: "unavailable" };
      const parsed = z
        .object({ invites: z.array(OrganizationInviteSchema) })
        .safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchOrganizationInvites", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return {
        kind: "ok",
        invites: parsed.data.invites.map((i) => ({
          ...i,
          role: asOrganizationRole(i.role),
        })),
      };
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchOrganizationInvites",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);
