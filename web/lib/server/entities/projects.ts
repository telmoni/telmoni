import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { compareProjects, compareProjectsByOrganization } from "@/lib/projects";
import { projectMatches } from "@/lib/slug";
import { asRole, type Role } from "@/lib/types/enums";

import { organizationHeaders, identityContext } from "./identity-context";

const ProjectSchema = z.object({
  id: z.string(),
  name: z.string(),
  slug: z.string().nullable().optional(),
  role: z.string(),
});

export type Project = { id: string; name: string; slug?: string | null; role: Role | null };

export type ProjectListing =
  | { kind: "ok"; projects: Project[] }
  | { kind: "unavailable" };

export const fetchProjectListing = cache(async (): Promise<ProjectListing> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/projects`, {
      headers: organizationHeaders(ctx),
    });
    if (!res.ok) return { kind: "unavailable" };
    const json = (await res.json()) as Record<string, unknown>;
    const rawList = Array.isArray(json?.projects) ? json.projects : [];
    const parsed = z.array(ProjectSchema).safeParse(rawList);
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchProjects", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      projects: parsed.data
        .map((t) => ({ ...t, role: asRole(t.role) }))
        .sort(compareProjects),
    };
  } catch {
    return { kind: "unavailable" };
  }
});

export const fetchProjects = cache(async (): Promise<Project[]> => {
  const listing = await fetchProjectListing();
  return listing.kind === "ok" ? listing.projects : [];
});

export const fetchProject = cache(
  async (projectId: string): Promise<Project | null> => {
    const list = await fetchProjects();
    return list.find((t) => projectMatches(t, projectId)) ?? null;
  },
);

const ProjectEverywhereSchema = ProjectSchema.extend({
  organizationId: z.string(),
  organizationName: z.string().nullable().optional(),
  organizationOwnerEmail: z.string().nullable().optional(),
});

/// A project in any organization the person can open, with what labels its
/// organization: the name its owner gave it, else the owner's address.
export type ProjectEverywhere = Project & {
  organizationId: string;
  organizationName: string | null;
  organizationOwnerEmail: string | null;
};

export type ProjectEverywhereListing =
  | { kind: "ok"; projects: ProjectEverywhere[] }
  | { kind: "unavailable" };

export const fetchProjectsEverywhere = cache(async (): Promise<ProjectEverywhereListing> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/projects/everywhere`, {
      headers: organizationHeaders(ctx),
    });
    if (!res.ok) return { kind: "unavailable" };
    const json = (await res.json()) as Record<string, unknown>;
    const parsed = z
      .array(ProjectEverywhereSchema)
      .safeParse(Array.isArray(json?.projects) ? json.projects : []);
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchProjectsEverywhere", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return {
      kind: "ok",
      projects: parsed.data
        .map((t) => ({
          ...t,
          role: asRole(t.role),
          organizationName: t.organizationName ?? null,
          organizationOwnerEmail: t.organizationOwnerEmail ?? null,
        }))
        .sort(compareProjectsByOrganization),
    };
  } catch {
    return { kind: "unavailable" };
  }
});
