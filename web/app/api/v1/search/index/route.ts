import type { NextRequest } from "next/server";

import { privateJson, privateProblem } from "@/lib/api/respond";
import { isProjectId } from "@/lib/ids";
import type { SearchIndex, SearchItem, SearchSection } from "@/lib/search";
import { admit } from "@/lib/server/admit";
import { fetchMembers, fetchProjectAnywhere, fetchTokens } from "@/lib/server/data";
import { projectPath } from "@/lib/slug";

// The search palette's index of a project — its API keys and its members,
// each leading to the page that lists it. A read, so a Route Handler: Server
// Actions are for writes, and the client sends them one at a time, so the
// palette opened during a slow save would wait on the save. Its answer is
// the session's own, so nothing in between keeps a copy.

const REFUSED: SearchSection = { items: [], forbidden: true, unavailable: false };
const BROKEN: SearchSection = { items: [], forbidden: false, unavailable: true };
const ok = (items: SearchItem[]): SearchSection => ({
  items,
  forbidden: false,
  unavailable: false,
});

export async function GET(request: NextRequest) {
  const gate = await admit({ call: "search:index", limit: 30 });
  if (!gate.ok) return privateProblem(gate.status, gate.error, gate.retryAfter);

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!isProjectId(projectId)) return privateProblem(404, "Unknown project.");

  // In the project's own organization: the path names none, and the cookie
  // follows whichever tab opened a page last.
  const project = await fetchProjectAnywhere(projectId);
  if (!project) return privateProblem(404, "Unknown project.");
  const page = (path: string) => projectPath(project.organizationSlug, project.slug, path);

  const [tokens, members] = await Promise.all([
    fetchTokens(projectId),
    fetchMembers(projectId),
  ]);

  const index: SearchIndex = {
    keys:
      tokens.kind === "ok"
        ? ok(
            tokens.tokens.map((t) => ({
              id: `key:${t.id}`,
              kind: "key" as const,
              label: t.name,
              hint: "API key",
              href: page("/api-keys"),
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
              href: page("/members"),
            })),
          )
        : members.kind === "forbidden"
          ? REFUSED
          : BROKEN,
  };
  return privateJson(index);
}
