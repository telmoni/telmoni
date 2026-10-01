"use server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { fetchMembers, fetchProject, fetchTokens } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
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

  if (!PROJECT_ID_RE.test(projectId)) return { error: "Unknown project." };

  const project = await fetchProject(projectId);
  if (!project) return { error: "Unknown project." };

  const [tokens, members] = await Promise.all([
    fetchTokens(projectId),
    fetchMembers(projectId),
  ]);

  return {
    keys:
      tokens.kind === "ok"
        ? ok(
            tokens.tokens.map((t) => ({
              id: `key:${t.id}`,
              kind: "key" as const,
              label: t.name,
              hint: "API key",
              href: `/${projectId}/api-keys`,
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
              href: `/${projectId}/members`,
            })),
          )
        : members.kind === "forbidden"
          ? REFUSED
          : BROKEN,
  };
}

