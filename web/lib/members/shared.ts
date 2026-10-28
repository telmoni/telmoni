import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem } from "@/lib/api/fetch";

/**
 * Safely extracts a shareable invite link from the upstream response.
 * Guard against `data.link` being null (which `String(null)` would turn into `"null"`).
 */
export function extractInviteLink(data: unknown): string | undefined {
  if (
    data &&
    typeof data === "object" &&
    "link" in data &&
    typeof (data as { link: unknown }).link === "string"
  ) {
    return (data as { link: string }).link;
  }
  return undefined;
}

/**
 * The roles an invitation or a role change may hand out. A Server Action takes
 * whatever is posted to it, whatever its parameter's type says, so the role
 * is checked here before it is sent anywhere; auth refuses anything else too.
 */
export function isGivenRole(role: unknown): role is "admin" | "member" {
  return role === "admin" || role === "member";
}

export const NOT_A_GIVEN_ROLE = "Choose admin or member.";

/**
 * Standard upstream error unwrapper for member management actions.
 */
export async function answerMemberAction(res: Response | null): Promise<ActionResult> {
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}
