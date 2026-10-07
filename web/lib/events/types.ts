import { z } from "zod";

import { isSlug } from "@/lib/slug";
import { IncomingInviteSchema } from "@/lib/types/incoming-invite";

const InviteRefSchema = z.object({ inviteId: z.string() });

const Slug = z.string().refine(isSlug);

/// An organization whose ownership offer or owner just changed, or that the
/// person just founded — or, when a project is named, one of whose projects
/// was offered, answered, or moved.
/// It carries no roles: the console re-reads `/me`, which is where
/// who-owns-what comes from. A moved project carries the slugs its new
/// address is spelled with, so a tab showing it can go there, by slug as every
/// link is; each is held to a slug's shape, because the listener spells a path
/// with it.
const OwnershipRefSchema = z.object({
  organizationId: z.string(),
  projectId: z.string().optional(),
  organizationSlug: Slug.optional(),
  projectSlug: Slug.optional(),
});

/// A project or organization from which the user's membership has been revoked.
const MembershipRemovedSchema = z.object({
  organizationId: z.string(),
  projectId: z.string().optional(),
});

/// A slug moved — an organization's URL changed on Settings, or a project
/// renamed — and with it the address of every page under it: an open tab
/// follows, where its path is spelled with `from`. The slugs are the
/// organization's own — or, when a project is named, that project's, in the
/// organization that goes by `organization`. Each is held to a slug's shape
/// here, because the listener spells a path with it.
const SlugMovedSchema = z.object({
  organizationId: z.string(),
  organization: Slug.optional(),
  projectId: z.string().optional(),
  from: Slug,
  to: Slug,
});

export const RealtimeEventSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("ownership:changed"),
    data: OwnershipRefSchema,
  }),
  z.object({
    type: z.literal("membership:removed"),
    data: MembershipRemovedSchema,
  }),
  z.object({
    type: z.literal("slug:moved"),
    data: SlugMovedSchema,
  }),
  z.object({
    type: z.literal("invite:created"),
    data: IncomingInviteSchema,
  }),
  z.object({
    type: z.literal("invite:sent"),
    data: InviteRefSchema,
  }),
  z.object({
    type: z.literal("invite:revoked"),
    data: InviteRefSchema,
  }),
  z.object({
    type: z.literal("invite:resolved"),
    data: InviteRefSchema,
  }),
]);

export type RealtimeEvent = z.infer<typeof RealtimeEventSchema>;

export const RealtimeEventDataSchema = {
  "ownership:changed": OwnershipRefSchema,
  "membership:removed": MembershipRemovedSchema,
  "slug:moved": SlugMovedSchema,
  "invite:created": IncomingInviteSchema,
  "invite:sent": InviteRefSchema,
  "invite:revoked": InviteRefSchema,
  "invite:resolved": InviteRefSchema,
} as const;
