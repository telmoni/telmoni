import "server-only";

import {
  fetchProjectBySlug,
  getServerContext,
  type Project,
  type ServerContext,
} from "@/lib/server/data";

/** What a project page renders under: the request's context and the project
 *  its path names, or `unavailable` when either cannot be read, an outage no
 *  page may read as a refusal. A slug naming no project is not found. */
export type ProjectPage =
  | { kind: "ok"; gate: ServerContext; project: Project }
  | { kind: "unavailable" };

export async function projectPage(slug: string): Promise<ProjectPage> {
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  return gate && project ? { kind: "ok", gate, project } : { kind: "unavailable" };
}
