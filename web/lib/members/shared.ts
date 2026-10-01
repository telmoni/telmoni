import { extractProblem } from "@/lib/api/fetch";

export interface ActionResult {
  error: string | null;
}

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
 * Standard upstream error unwrapper for member management actions.
 */
export async function answerMemberAction(res: Response | null): Promise<ActionResult> {
  if (!res) {
    return { error: "Organizations are unavailable right now — try again shortly." };
  }
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { error: null };
}
