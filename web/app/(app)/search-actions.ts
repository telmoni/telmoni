"use server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { activeOrganization, fetchMembers, fetchProject, fetchTokens, getServerContext, identityContext } from "@/lib/server/data";
import { organizationSegment } from "@/lib/slug";
import { getServerSession } from "@/lib/server/session";
import { isValidSlug, projectSegment } from "@/lib/slug";
import type { SearchItem } from "@/lib/search";

export interface SearchSection {
  items: SearchItem[];
  forbidden: boolean;
  unavailable: boolean;
}

export interface SearchIndex {
  keys: SearchSection;
  members: SearchSection;
}

const REFUSED: SearchSection = { items: [], forbidden: true, unavailable: false };
const BROKEN: SearchSection = { items: [], forbidden: false, unavailable: true };
const ok = (items: SearchItem[]): SearchSection => ({
  items,
  forbidden: false,
  unavailable: false,
});

const PROJECT_ID_RE = /^project_[0-9A-Za-z]{16}$/;

export async function searchIndexAction(
  projectId: string,
): Promise<SearchIndex | { error: string }> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };

  const limited = await rateLimit(sessionKey(session, "search:index"), {
    limit: 30,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  if (!PROJECT_ID_RE.test(projectId) && !isValidSlug(projectId)) return { error: "Unknown project." };

  const [ctx, project, serverCtx] = await Promise.all([
    identityContext(),
    fetchProject(projectId),
    getServerContext(),
  ]);
  if (!project) return { error: "Unknown project." };

  const [tokens, members] = await Promise.all([
    fetchTokens(project.id),
    fetchMembers(project.id),
  ]);

  const activeOrg = serverCtx ? activeOrganization(serverCtx) : null;
  const orgSeg = activeOrg ? organizationSegment(activeOrg) : ctx?.organizationId;
  const orgPrefix = orgSeg ? `/${orgSeg}` : "";
  const projectTarget = projectSegment(project);

  return {
    keys:
      tokens.kind === "ok"
        ? ok(
            tokens.tokens.map((t) => ({
              id: `key:${t.id}`,
              kind: "key" as const,
              label: t.name,
              hint: "API key",
              href: `${orgPrefix}/${projectTarget}/api-keys`,
            })),
          )
        : tokens.kind === "forbidden"
          ? REFUSED
          : BROKEN,

    members:
      members.kind === "ok"
        ? ok(
            members.members.map((m) => ({
              id: `member:${m.member_id}`,
              kind: "member" as const,
              label: m.email,
              hint: m.display_name ?? undefined,
              href: `${orgPrefix}/${projectTarget}/members`,
            })),
          )
        : members.kind === "forbidden"
          ? REFUSED
          : BROKEN,
  };
}
