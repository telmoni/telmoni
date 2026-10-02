import { consolePlace } from "./console-nav";
import { organizationPath, projectPath } from "./slug";

export const SHORTCUTS_DISABLED_KEY = "telmoni-shortcuts-disabled";

export const SHORTCUTS_SHEET_EVENT = "telmoni:shortcuts-sheet";

export const SEARCH_OPEN_EVENT = "telmoni:search-open";

export const SEARCH_KEY = "/";

// ⚠ **⌘K is the search key everywhere else, so it is the search key here.**
// The account menu uses ⌘⇧K — see `nav-user.tsx`.
export const SEARCH_MODIFIER_KEY = "k";

// The header's agent button: opens the panel, or closes it when it is open.
export const AGENT_TOGGLE_EVENT = "telmoni:agent-toggle";

// ⌘J, beside ⌘K: the two are the console's ways to ask it something, and a
// hand on one finds the other.
export const AGENT_MODIFIER_KEY = "j";

export const ACCOUNT_MENU_EVENT = "telmoni:account-menu";

export const SEQUENCE_TIMEOUT_MS = 1000;

export const PAGE_ACTION_KEY = "c";

interface GoSequence {
  key: string;
  label: string;
  href: string;
  // Goes to the organization's overview, whatever the path stands in.
  organization?: boolean;
}

export const GO_SEQUENCES: GoSequence[] = [
  { key: "k", label: "API keys", href: "/api-keys" },
  // `c` is also `PAGE_ACTION_KEY`, and that is safe rather than a collision:
  // the handler takes the pending-`g` branch and returns before it ever reaches
  // the page-action check, so the two readings of the key never meet. It is the
  // only key in this table with a bare meaning as well, so the ordering there is
  // load-bearing — `keyboard-shortcuts.test.tsx` pins both halves.
  { key: "c", label: "Connectors", href: "/connectors" },
  { key: "m", label: "Members", href: "/members" },
  { key: "l", label: "Audit log", href: "/audit-log" },
  { key: "p", label: "Project settings", href: "/settings" },
  { key: "o", label: "Overview", href: "" },
  { key: "O", label: "Organization", href: "", organization: true },
];

const ORGANIZATION_PAGES = new Set(["", "/members", "/audit-log", "/settings"]);

// Where a sequence goes from `pathname`. All but `g O` are relative to the
// resource the path stands in: a project has every one of them, the
// organization has its own spellings of Overview, Members, Audit log and
// Settings, and Account has none. `organization` is the slug of the
// organization the console stands in, for `g O` from a path that names none.
//
// ⚠ **Returning a bare `seq.href` with nowhere to stand was a 404 generator.**
// `/api-keys` unprefixed reads as an organization of that name, and its layout
// answers `notFound()`. A shortcut with nowhere to go must do nothing instead.
export function resolveSequence(
  key: string,
  pathname: string,
  organization?: string | null,
): string | null {
  const seq = GO_SEQUENCES.find((s) => s.key === key);
  if (seq === undefined) return null;

  const place = consolePlace(pathname);
  const standing = place !== null && place.kind !== "account" ? place : null;
  if (seq.organization) {
    const slug = standing?.organization ?? organization;
    return slug ? organizationPath(slug) : null;
  }
  if (standing === null) return null;
  if (standing.kind === "organization") {
    return ORGANIZATION_PAGES.has(seq.href)
      ? organizationPath(standing.organization, seq.href)
      : null;
  }
  return projectPath(standing.organization, standing.project, seq.href);
}
