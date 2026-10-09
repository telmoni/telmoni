import { buildConsoleNav, consolePlace, type ConsoleNavItem } from "./console-nav";
import type { FlagSet } from "./flags";
import { organizationPath } from "./slug";

export const SHORTCUTS_SHEET_EVENT = "telmoni:shortcuts-sheet";

export const SEARCH_OPEN_EVENT = "telmoni:search-open";

export const SEARCH_KEY = "/";

// ⚠ **⌘K is the search key everywhere else, so it is the search key here.**
// The account menu uses ⌘⇧K — see `nav-user.tsx`.
export const SEARCH_MODIFIER_KEY = "k";

// The header's agent button: opens the agent's window, or closes it when it is open.
export const AGENT_TOGGLE_EVENT = "telmoni:agent-toggle";

// What stands in front of the page and takes its keys and its presses: a
// modal dialog, an alert, a menu. One list for every reader — the letter keys,
// the rail's drawer, ⌘J — which must agree about what counts as in front.
// ⚠ **The agent's window is a dialog that does not count**: it floats beside
// the page without shutting it off, and says so with `aria-modal="false"`
// (`components/agent/agent-window.tsx`), so the page keeps its keys while it
// is open. A dialog that shuts the page off never says that.
export const MODAL_SELECTOR =
  '[role="dialog"]:not([aria-modal="false"]), [role="alertdialog"], [role="menu"]';

// ⌘J, beside ⌘K: the two are the console's ways to ask it something, and a
// hand on one finds the other.
export const AGENT_MODIFIER_KEY = "j";

export const ACCOUNT_MENU_EVENT = "telmoni:account-menu";

export const SEQUENCE_TIMEOUT_MS = 1000;

export const PAGE_ACTION_KEY = "c";

/** `g` then this reaches the organization's overview from anywhere, Account
 *  included: the one sequence that is not a row of the rail you stand in. */
export const ORGANIZATION_KEY = "O";

export interface GoSequence {
  key: string;
  label: string;
  href: string;
}

// The `g` sequences are the rail's rows: each row that carries a `key` in
// `lib/console-nav.ts` is reached by `g` and that key, so a row added to the
// rail — a console built on this one adds its own through
// `lib/extension/nav.ts` — is reached the day it lands, and the sheet lists
// exactly the rail you stand in, which keeps a sequence from ever promising
// a page the rail does not have. Account's rows are reached the same way.
// `c` is also `PAGE_ACTION_KEY`, and that is safe rather than a collision:
// the handler takes the pending-`g` branch and returns before it ever reaches
// the page-action check, so the two readings of the key never meet.
//
// ⚠ **A sequence with nowhere to go must do nothing.** A bare project-relative
// href — `/api-keys` — reads as an organization of that name and 404s. Off
// the console's paths the rail is empty, so nothing resolves there but `g O`
// with an organization to go to.
export function goSequences(
  pathname: string,
  organization?: string | null,
  flags: FlagSet = {},
): GoSequence[] {
  const rows = buildConsoleNav(pathname, flags)
    .flatMap((g) => g.items)
    .filter((item): item is ConsoleNavItem & { key: string } => item.key !== undefined)
    .map((item) => ({ key: item.key, label: item.title, href: item.url }));
  const place = consolePlace(pathname);
  const slug = place !== null && place.kind !== "account" ? place.organization : organization;
  if (slug) {
    rows.push({
      key: ORGANIZATION_KEY,
      label: "The organization's overview",
      href: organizationPath(slug),
    });
  }
  return rows;
}

/** Where `g` and `key` go from `pathname`; `null` is nowhere, and nothing. */
export function resolveSequence(
  key: string,
  pathname: string,
  organization?: string | null,
  flags: FlagSet = {},
): string | null {
  return goSequences(pathname, organization, flags).find((s) => s.key === key)?.href ?? null;
}
