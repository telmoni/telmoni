import { getServerContext } from "@/lib/server/entities/organization";

/// What an action answers when `identityContext` could place the person in no
/// organization. The context tells the two causes apart: the path the action
/// was posted from names an organization auth did not answer with — its URL
/// changed since the page rendered, or one the person is no longer in — which
/// is a tab that missed the move, and a reload would only answer "not found";
/// or `/me` itself did not answer, which a moment may mend.
export async function unplacedOrganization(): Promise<string> {
  const ctx = await getServerContext();
  return ctx?.organizationNotFound
    ? "Nothing at this address any more: the organization's URL was changed, or you're no longer in it. Find it in the organization menu."
    : "Couldn't resolve your organization right now. Try again in a moment.";
}

/// What an action that changes an organization answers when the organization
/// its page rendered is not the one the action resolves.
///
/// ⚠ **An action acts in the organization its request resolves, which is the
/// page's only while the page is current.** On an organization's own path the
/// slug decides, and an organization whose URL changed since the page rendered
/// goes by a new one: the old path names nobody, and auth falls back to
/// another the person is in. Off it — Account, a console built on this one's
/// own pages — the cookie decides, and another tab moves that. Either way the
/// click would
/// land on an organization the page is not showing: "Send confirmation code"
/// on A's settings mints a code bound to B, and typing it deletes B while the
/// page still says A. So each of these actions takes the id its page rendered,
/// and refuses before it asks any service anything when the two differ.
export const SWITCHED_ORGANIZATION =
  "This page is out of date. Reload it to act on the organization you're viewing.";
