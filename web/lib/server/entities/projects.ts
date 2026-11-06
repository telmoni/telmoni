import { cache } from "react";
import { notFound } from "next/navigation";
import { z } from "zod";

import { env } from "@/lib/env";
import { pathOf } from "@/lib/console-nav";
import { compareProjects, compareProjectsByOrganization } from "@/lib/projects";
import { ContentMode, Role } from "@/lib/types/enums";

import { organizationHeaders, identityContext } from "./identity-context";
import { getServerContext } from "./organization";
import { readJson } from "./read";

export const ProjectSchema = z.object({
  id: z.string(),
  /// Its segment under the organization's: `/{organization}/{slug}`.
  slug: z.string(),
  name: z.string(),
  role: z.enum(Role),
  /// Whether its spans' content is kept, as its menu row shows.
  content_mode: z.enum(ContentMode),
});

export type Project = z.infer<typeof ProjectSchema>;

// A project renamed: the slug its pages go by now.
export const ProjectRenamedSchema = z.object({ slug: z.string() });

// How many projects the organization may hold, as the deployment that sells
// plans answers it, how many it holds, whether one more would be refused now,
// the sentence it is refused with, and where what lifts it is bought, read as
// the path the router takes; `null` where none is held, as on a self-hosted
// installation.
export const ProjectCeilingSchema = z.object({
  limit: z.number().int().min(0),
  held: z.number().int().min(0),
  reached: z.boolean(),
  detail: z.string(),
  page: z.string().nullable(),
});
export type ProjectCeiling = z.infer<typeof ProjectCeilingSchema>;

export type ProjectListing =
  | { kind: "ok"; projects: Project[]; ceiling: ProjectCeiling | null }
  | { kind: "unavailable" };

// The envelope is the schema's: an answer without its list is unreadable,
// never an organization with no projects.
export const ProjectListSchema = z.object({
  projects: z.array(ProjectSchema),
  ceiling: ProjectCeilingSchema.nullable(),
});

export const fetchProjectListing = cache(async (): Promise<ProjectListing> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchProjects",
    `${env.SERVER_URL}/internal/v1/projects`,
    { headers: organizationHeaders(ctx) },
    ProjectListSchema,
  );
  if (read.kind !== "ok") return { kind: "unavailable" };
  return {
    kind: "ok",
    projects: read.value.projects.sort(compareProjects),
    ceiling: read.value.ceiling && {
      ...read.value.ceiling,
      page: read.value.ceiling.page === null ? null : pathOf(read.value.ceiling.page),
    },
  };
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

const ProjectEverywhereSchema = ProjectSchema.omit({ content_mode: true }).extend({
  organizationId: z.string(),
  organizationSlug: z.string(),
  organizationName: z.string(),
});

/// A project in any organization the person can open, with where its
/// organization's paths begin and what labels it: the name it was given.
export type ProjectEverywhere = Omit<Project, "content_mode"> & {
  organizationId: string;
  organizationSlug: string;
  organizationName: string;
};

export type ProjectEverywhereListing =
  | { kind: "ok"; projects: ProjectEverywhere[] }
  | { kind: "unavailable" };

export const ProjectEverywhereListSchema = z.object({
  projects: z.array(ProjectEverywhereSchema),
});

export const fetchProjectsEverywhere = cache(async (): Promise<ProjectEverywhereListing> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchProjectsEverywhere",
    `${env.SERVER_URL}/internal/v1/projects/everywhere`,
    { headers: organizationHeaders(ctx) },
    ProjectEverywhereListSchema,
  );
  if (read.kind !== "ok") return { kind: "unavailable" };
  return { kind: "ok", projects: read.value.projects.sort(compareProjectsByOrganization) };
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
