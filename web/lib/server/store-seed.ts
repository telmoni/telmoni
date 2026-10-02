import { cache } from "react";

import type { StoreInitial } from "@/lib/store/types";
import { Role } from "@/lib/types/enums";

import { identityContext } from "./entities/identity-context";
import { getServerContext } from "./entities/organization";
import { fetchProjects, fetchProjectsEverywhere } from "./entities/projects";
import { getServerSession } from "./session";

/// What the console's store holds for the organization this request acts in;
/// `null` without a session.
///
/// Two layouts seed the store with it. `(app)`'s is shared by every
/// organization's pages, so the router keeps it across a move from one
/// organization to another and its seed goes stale; `[organization]`'s
/// re-renders on that move, and hands over the seed for the one arrived in.
export const storeSeed = cache(async (): Promise<StoreInitial | null> => {
  const [session, ctx, ident, projects, everywhere] = await Promise.all([
    getServerSession(),
    getServerContext(),
    identityContext(),
    fetchProjects(),
    fetchProjectsEverywhere(),
  ]);
  if (!session) return null;

  const roles: Record<string, Role> = Object.fromEntries(
    projects
      .filter((p): p is typeof p & { role: Role } => p.role !== null)
      .map((p) => [p.id, p.role]),
  );
  const organizationRole = ident?.role ?? null;
  if (organizationRole) {
    roles.organization = {
      owner: Role.Owner,
      admin: Role.Admin,
      member: Role.Member,
    }[organizationRole];
  }

  const activeOrganizationId = ident?.organizationId ?? null;
  return {
    user: {
      id: session.userId,
      email: session.email,
      firstName: session.firstName,
      lastName: session.lastName,
    },
    organizations: ctx?.organizations ?? [],
    incomingInvites: ctx?.incomingInvites ?? [],
    projectOffers: ctx?.projectOffers ?? [],
    activeOrganizationId,
    flags: ctx?.flags ?? {},
    roles,
    projects,
    projectsElsewhere:
      everywhere.kind === "ok" && activeOrganizationId
        ? everywhere.projects.filter((p) => p.organizationId !== activeOrganizationId)
        : [],
  };
});
