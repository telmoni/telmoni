"use server";

import { redirect } from "next/navigation";
import { revalidatePath } from "next/cache";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { fetchProject, identityContext, projectHeaders } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

// Mark this PROJECT's feed read. The organization's feed is marked on its own,
// and the two clear different sets on purpose — the service
// picks its scope from `x-project-id`, so a badge cleared in one scope was never
// counting the other's rows.
export async function markProjectReadAction(
  projectId: string,
): Promise<{ error: string | null }> {
  const session = await getServerSession();
  if (!session) redirect("/auth/login");

  const limited = await rateLimit(sessionKey(session, "notifications:read"), {
    limit: 20,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };

  const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
  // Re-resolved here rather than trusted from the argument: a Server Action is
  // a public endpoint, so the project a caller names is a claim until a membership
  // row answers for it.
  if (!ctx || !project) {
    return { error: "Couldn't resolve that project. Reload and try again." };
  }

  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/notifications/read`,
    { method: "POST", headers: projectHeaders(ctx, project.id) },
  );
  if (!res) {
    return {
      error: "Notifications are unavailable right now — try again shortly.",
    };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };

  if (ctx.organizationId) {
    revalidatePath(`/${ctx.organizationId}/${projectId}`);
  }
  revalidatePath(`/${projectId}`);
  return { error: null };
}
