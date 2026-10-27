import { type NextRequest, NextResponse } from "next/server";

import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import type { SearchIndex, SearchItem, SearchSection } from "@/lib/search";
import { fetchMembers, fetchProjectAnywhere, fetchTokens } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { projectPath } from "@/lib/slug";

// The search palette's index of a project — its API keys and its members,
// each leading to the page that lists it. A read, so a Route Handler: Server
// Actions are for writes, and the client sends them one at a time, so the
// palette opened during a slow save would wait on the save. Its answer is
// the session's own, so nothing in between keeps a copy.

const NO_STORE = { "cache-control": "private, no-store" };

const REFUSED: SearchSection = { items: [], forbidden: true, unavailable: false };
const BROKEN: SearchSection = { items: [], forbidden: false, unavailable: true };
const ok = (items: SearchItem[]): SearchSection => ({
  items,
  forbidden: false,
  unavailable: false,
});

const PROJECT_ID_RE = /^project_[0-9A-Za-z]{16}$/;

function refuse(error: string, status: number) {
  return NextResponse.json({ error }, { status, headers: NO_STORE });
}

export async function GET(request: NextRequest) {
  const session = await getServerSession();
  if (!session) return refuse("Your session expired — sign in again.", 401);

  const limited = await rateLimit(sessionKey(session, "search:index"), {
    limit: 30,
    windowMs: 60_000,
  });
  if (limited) return refuse("Too many requests — slow down a moment.", 429);

  const projectId = request.nextUrl.searchParams.get("project") ?? "";
  if (!PROJECT_ID_RE.test(projectId)) return refuse("Unknown project.", 404);

  // In the project's own organization: the path names none, and the cookie
  // follows whichever tab opened a page last.
  const project = await fetchProjectAnywhere(projectId);
  if (!project) return refuse("Unknown project.", 404);
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
  return NextResponse.json(index, { headers: NO_STORE });
}
