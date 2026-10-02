import { cache } from "react";
import { notFound } from "next/navigation";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { compareProjects, compareProjectsByOrganization } from "@/lib/projects";
import { asRole, type Role } from "@/lib/types/enums";

import { organizationHeaders, identityContext } from "./identity-context";
import { getServerContext } from "./organization";

const ProjectSchema = z.object({
  id: z.string(),
  /// Its segment under the organization's: `/{organization}/{slug}`.
  slug: z.string(),
  name: z.string(),
  role: z.string(),
});

export type Project = { id: string; slug: string; name: string; role: Role | null };

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

/// A project of the organization the request acts in, by id.
export const fetchProject = cache(
  async (projectId: string): Promise<Project | null> => {
    const list = await fetchProjects();
    return list.find((t) => t.id === projectId) ?? null;
  },
);

/// The project a page's path names, by its slug in the organization the path
/// names — the one the request acts in, which is the listing's. `null` when
/// the listing cannot be read. Not found when the path names an organization
/// auth did not answer with, or one that holds no project by that slug: a
/// rename, a delete or a transfer since the link was drawn. Asked by every
/// project page on every render, since the layout above it is kept across a
/// move between a project's pages and does not check again.
export const fetchProjectBySlug = cache(
  async (slug: string): Promise<Project | null> => {
    const ctx = await getServerContext();
    if (ctx?.organizationNotFound) notFound();
    const listing = await fetchProjectListing();
    if (listing.kind !== "ok") return null;
    const project = listing.projects.find((t) => t.slug === slug);
    if (!project) notFound();
    return project;
  },
);

const ProjectEverywhereSchema = ProjectSchema.extend({
  organizationId: z.string(),
  organizationSlug: z.string(),
  organizationName: z.string().nullable().optional(),
  organizationOwnerEmail: z.string().nullable().optional(),
});

/// A project in any organization the person can open, with where its
/// organization's paths begin and what labels it: the name its owner gave it,
/// else the owner's address.
export type ProjectEverywhere = Project & {
  organizationId: string;
  organizationSlug: string;
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

/// A project the person can open, in whichever organization holds it, by id:
/// what a route handler resolves the project it is handed with. Its path
/// names no organization, so the request stands wherever the cookie last
/// pointed, which another tab moves; the project's own organization is the
/// one to act in.
export const fetchProjectAnywhere = cache(
  async (projectId: string): Promise<ProjectEverywhere | null> => {
    const everywhere = await fetchProjectsEverywhere();
    if (everywhere.kind !== "ok") return null;
    return everywhere.projects.find((p) => p.id === projectId) ?? null;
  },
);
