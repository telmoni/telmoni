import { identityContext } from "@/lib/server/entities/identity-context";
import { fetchProjects } from "@/lib/server/data";
import { projectMatches } from "@/lib/slug";

/// What an action that changes an organization answers when the organization
/// its page rendered is no longer the one the session stands in.
///
/// ⚠ **An action resolves the active organization when it is CALLED, from a
/// cookie every tab shares — and another tab can have switched it since this
/// page rendered.** Acting on that resolution aims the click at an organization
/// the page is not showing: "Send confirmation code" on A's settings mints a
/// code bound to B, and typing it deletes B while the page still says A. So
/// each of these actions takes the id its page rendered, and refuses before it
/// asks any service anything when the two differ.
export const SWITCHED_ORGANIZATION =
  "You switched organizations in another tab. Reload this page to act on the one you're viewing.";

/// The project a key lane acts on, with the organization the session stands
/// in. The project is the one its page rendered, and auth takes its
/// organization from the project itself, so a switch in another tab cannot
/// aim a key at a project the page is not showing.
export async function activeProjectForMutation(projectId: string): Promise<
  | { organizationId: string; projectId: string; error: null }
  | { organizationId?: undefined; projectId?: undefined; error: string }
> {
  if (!projectId) {
    return { error: "Couldn't tell which project this is. Reload and try again." };
  }
  const ident = await identityContext();
  if (!ident) {
    return { error: "Couldn't resolve your organization right now. Try again in a moment." };
  }
  let resolvedId = projectId;
  if (!projectId.startsWith("project_")) {
    const projects = await fetchProjects();
    const matched = projects.find((p) => projectMatches(p, projectId));
    if (matched) {
      resolvedId = matched.id;
    }
  }
  return { organizationId: ident.organizationId, projectId: resolvedId, error: null };
}
