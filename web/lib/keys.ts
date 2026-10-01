
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
  absolute?: boolean;
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
  { key: "O", label: "Organization", href: "/organization", absolute: true },
];

const ORGANIZATION_PAGES = new Set(["", "/members", "/audit-log", "/settings"]);

// Every first path segment under `app/(app)` that is NOT a project id. The rail
// makes the same split — `buildConsoleNav` draws the Organization run for the
// first and the Account run for the second, never a project's — and a
// project-relative sequence has no destination from either.
// `console` is here too: it is a redirector, and it is where a sign-in lands
// before the project listing resolves.
const NON_PROJECT_ROOTS = new Set(["organization", "account", "console"]);

// `root` is what `useSelectedLayoutSegment()` returns under the console layout
// — a project id, "organization" or "account" — and not `useParams().projectId`: the
// latter two are literal segments with no `[projectId]` to read, and the
// organization has its own spellings of Members, Audit log and Settings.
// `null` is the layout with nothing selected under it.
//
// ⚠ **Returning a bare `seq.href` when the root is unknown was a 404
// generator.** `/api-keys`, `/connectors`, `/members`, `/audit-log` and `/settings`
// are all project-relative; unprefixed they match `[projectId]` with the page name
// AS the project id, and `[projectId]/layout.tsx` answers `notFound()`. A shortcut
// with nowhere to go must do nothing instead.
export function resolveSequence(
  key: string,
  root?: string | null,
): string | null {
  const seq = GO_SEQUENCES.find((s) => s.key === key);
  if (seq === undefined) return null;
  if (seq.absolute) return seq.href;
  if (root === "organization") {
    return ORGANIZATION_PAGES.has(seq.href) ? `/organization${seq.href}` : null;
  }
  if (!root || NON_PROJECT_ROOTS.has(root)) return null;
  return `/${root}${seq.href}`;
}
