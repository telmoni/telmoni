import { parseConsolePath } from "./console-nav";

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
// `root` is what `useSelectedLayoutSegment()` returns under the console layout
// — a project id, "organization" or "account" — or the current pathname, or an
// explicit `{ orgId, projectId }` object.
export function resolveSequence(
  key: string,
  root?: string | { organizationId?: string | null; orgId?: string | null; projectId?: string | null } | null,
): string | null {
  const seq = GO_SEQUENCES.find((s) => s.key === key);
  if (seq === undefined) return null;

  let orgId: string | null = null;
  let projectId: string | null = null;
  let mode: "account" | "organization" | "project" | "unknown" = "unknown";

  if (typeof root === "string") {
    if (root.startsWith("/")) {
      const parsed = parseConsolePath(root);
      orgId = parsed.orgId;
      projectId = parsed.projectId;
      mode = parsed.mode;
    } else if (root === "organization" || root.startsWith("org_") || root === "org") {
      orgId = root;
      mode = "organization";
    } else if (root === "account" || root === "console") {
      mode = "account";
    } else if (root) {
      projectId = root;
      mode = "project";
    }
  } else if (root) {
    orgId = root.organizationId ?? root.orgId ?? null;
    projectId = root.projectId ?? null;
    mode = projectId ? "project" : orgId ? "organization" : "unknown";
  }

  if (seq.key === "O") {
    return orgId && orgId !== "organization" ? `/${orgId}` : "/organization";
  }
  if (seq.absolute) return seq.href;

  if (mode === "organization") {
    if (ORGANIZATION_PAGES.has(seq.href)) {
      return orgId && orgId !== "organization"
        ? `/${orgId}${seq.href}`
        : `/organization${seq.href}`;
    }
    return null;
  }

  if (mode === "project") {
    if (!projectId) return null;
    return orgId ? `/${orgId}/${projectId}${seq.href}` : `/${projectId}${seq.href}`;
  }

  return null;
}
