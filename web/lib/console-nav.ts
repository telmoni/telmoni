import {
  Accessibility,
  Activity,
  Bell,
  BellRing,
  History,
  KeyRound,
  LayoutDashboard,
  ListTree,
  MessagesSquare,
  Plug,
  Settings,
  ShieldCheck,
  SquareStack,
  User,
  type LucideIcon,
  Users,
  UsersRound,
} from "lucide-react";

import { EXTRA_NAV_ITEMS, type ExtraNavItem } from "@/lib/extension/nav";
import { Flag, type FlagSet, allOff } from "@/lib/flags";
import {
  isOrganizationPage,
  isOrganizationSlug,
  organizationPath,
  projectPath,
  withLeadingSegments,
} from "@/lib/slug";

export const NAV_COLLAPSED_COOKIE = "telmoni-nav-collapsed";

export const ACCOUNT_SEGMENT = "account";

export const NOTIFICATIONS_HREF = "/account/notifications";

export function rootSegment(pathname: string): string {
  return pathname.split("/")[1] ?? "";
}

export function isAccountPath(pathname: string): boolean {
  return rootSegment(pathname) === ACCOUNT_SEGMENT;
}

/**
 * Where a console path stands, read off its segments alone (`lib/slug.ts` has
 * the scheme). `null` for a path that names no organization: `/console`, an
 * `/api` route, the site's own pages.
 */
export type ConsolePlace =
  | { kind: "account" }
  | { kind: "organization"; organization: string }
  | { kind: "project"; organization: string; project: string };

export function consolePlace(pathname: string): ConsolePlace | null {
  const [organization = "", second = ""] = pathname.split("/").slice(1);
  if (organization === ACCOUNT_SEGMENT) return { kind: "account" };
  if (!isOrganizationSlug(organization)) return null;
  if (second === "" || isOrganizationPage(second)) {
    return { kind: "organization", organization };
  }
  return { kind: "project", organization, project: second };
}

/**
 * The project `pathname` names, out of `projects`: the listing of the
 * organization that goes by `organization`. `null` off a project's pages, and
 * while the listing is another organization's than the path names.
 */
export function projectAt<P extends { slug: string }>(
  pathname: string,
  organization: string | null,
  projects: readonly P[],
): P | null {
  const place = consolePlace(pathname);
  if (place?.kind !== "project" || place.organization !== organization) return null;
  return projects.find((p) => p.slug === place.project) ?? null;
}

/**
 * The organization `pathname` names when the console's shell was rendered for
 * another one, `rendered`; `null` when the two agree, or the path names none.
 *
 * Only an organization the person is in counts: auth answers a path that names
 * any other with one of theirs, and asking again would get the same answer.
 */
export function staleShellOrganization(
  pathname: string,
  rendered: string | null,
  organizations: readonly { slug: string }[],
): string | null {
  const place = consolePlace(pathname);
  if (place === null || place.kind === "account") return null;
  const named = place.organization;
  if (named === rendered || !organizations.some((o) => o.slug === named)) return null;
  return named;
}

/**
 * Where `pathname` is now that a slug it is spelled with has moved: an
 * organization's URL changed (`from` to `to`), or, when `organization` names
 * the one a project is in, that project renamed. `null` when the path is not
 * under it.
 */
export function movedPath(
  pathname: string,
  moved: { organization?: string; from: string; to: string },
): string | null {
  const place = consolePlace(pathname);
  if (place === null || place.kind === "account") return null;
  if (moved.organization === undefined) {
    return place.organization === moved.from ? withLeadingSegments(pathname, [moved.to]) : null;
  }
  return place.kind === "project" &&
    place.organization === moved.organization &&
    place.project === moved.from
    ? withLeadingSegments(pathname, [moved.organization, moved.to])
    : null;
}

/**
 * The id of the organization to remember for the paths that name none (the
 * cookie in `lib/proxy/organization.ts`): the one `pathname` names, when the
 * person is in it; off an organization's path, the one the console stands in,
 * `active`. `null` when there is none to remember — a path that names
 * somebody else's.
 */
export function organizationToRemember(
  pathname: string,
  organizations: readonly { organizationId: string; slug: string }[],
  active: string | null,
): string | null {
  const place = consolePlace(pathname);
  if (place === null || place.kind === "account") return active;
  return organizations.find((o) => o.slug === place.organization)?.organizationId ?? null;
}

/**
 * A row that opens a block of the rail: the rows from it to the next such row
 * stand together, clear of the block above, under `heading` when there is
 * one. A run's first row opens its first block whether it says so or not.
 */
export interface NavBlock {
  heading?: string;
}

export interface ConsoleNavItem {
  title: string;
  url: string;
  icon: LucideIcon;
  isActive: boolean;
  block?: NavBlock;
  /** The letter after `g` that reaches the row (`lib/keys.ts`). */
  key?: string;
}

export interface ConsoleNavGroup {
  title: string;
  items: ConsoleNavItem[];
}

export interface ConsoleNavBlock {
  heading?: string;
  items: ConsoleNavItem[];
}

/** A run's rows in their blocks, for the rail to draw. */
export function navBlocks(items: readonly ConsoleNavItem[]): ConsoleNavBlock[] {
  const blocks: ConsoleNavBlock[] = [];
  let current: ConsoleNavBlock | undefined;
  for (const item of items) {
    if (item.block || !current) {
      current = { heading: item.block?.heading, items: [] };
      blocks.push(current);
    }
    current.items.push(item);
  }
  return blocks;
}

interface GroupSpec {
  title: string;
  items: {
    title: string;
    path: string;
    icon: LucideIcon;
    flags?: readonly Flag[];
    block?: NavBlock;
    key?: string;
  }[];
}

// `key` is the letter after `g` that reaches a row, so the `g` sequences are
// the rail itself and a row added here is reached the day it lands. Single,
// unique within a rail (two rails may share one: `p` is Projects on an
// organization's and Privacy on Account's), never `g`, `?` or shift-`O`, the
// one sequence that crosses rails (`lib/keys.ts`). `,` reaches Settings on
// every rail, as `⌘,` opens preferences on a Mac; `c` is Connectors and also
// the page's action, which `lib/keys.ts` explains is no collision.

const CORE_GROUPS: GroupSpec[] = [
  {
    title: "Project",
    // Dashboards, Traces, Sessions, Users and Alerts stand in the rail before
    // their pages exist, so the console shows the product's shape while
    // telemetry is built: each opens a placeholder that the real page
    // replaces. The Observability
    // block is headed because its five rows are the product, and the Project
    // block because the rest of the rail is the project's own housekeeping;
    // Overview stands alone above both.
    items: [
      { title: "Overview", path: "", icon: Activity, key: "o" },
      { title: "Traces", path: "/traces", icon: ListTree, block: { heading: "Observability" }, key: "t" },
      { title: "Sessions", path: "/sessions", icon: MessagesSquare, key: "s" },
      { title: "Users", path: "/users", icon: UsersRound, key: "u" },
      { title: "Alerts", path: "/alerts", icon: BellRing, key: "a" },
      { title: "Dashboards", path: "/dashboards", icon: LayoutDashboard, key: "d" },
      { title: "API keys", path: "/api-keys", icon: KeyRound, block: { heading: "Project" }, key: "k" },
      { title: "Connectors", path: "/connectors", icon: Plug, key: "c" },
      { title: "Members", path: "/members", icon: Users, key: "m" },
      { title: "Audit log", path: "/audit-log", icon: History, key: "l" },
      { title: "Settings", path: "/settings", icon: Settings, key: "," },
    ],
  },
  {
    title: "Organization",
    // Overview stands alone; the organization's own rows are headed, as a
    // project's housekeeping is headed Project.
    items: [
      { title: "Overview", path: "", icon: Activity, key: "o" },
      { title: "Projects", path: "/projects", icon: SquareStack, block: { heading: "Organization" }, key: "p" },
      { title: "Members", path: "/members", icon: Users, key: "m" },
      { title: "Audit log", path: "/audit-log", icon: History, key: "l" },
      { title: "Settings", path: "/settings", icon: Settings, key: "," },
    ],
  },
  {
    title: "Account",
    // One unheaded block: the way back under it says what this rail is.
    items: [
      { title: "Settings", path: "/settings", icon: User, key: "," },
      { title: "Notifications", path: "/notifications", icon: Bell, key: "n" },
      { title: "Accessibility", path: "/accessibility", icon: Accessibility, key: "a" },
      { title: "Privacy", path: "/privacy", icon: ShieldCheck, key: "p" },
    ],
  },
];

/** The core's rows with a console built on this one's joined in. */
export function withExtraItems(
  groups: readonly GroupSpec[],
  extras: readonly ExtraNavItem[],
): GroupSpec[] {
  return groups.map((group) => {
    const items = [...group.items];
    for (const extra of extras) {
      if (extra.group !== group.title) continue;
      const row = {
        title: extra.title,
        path: extra.path,
        icon: extra.icon,
        ...(extra.key ? { key: extra.key } : {}),
      };
      const at = extra.before
        ? items.findIndex((item) => item.title === extra.before)
        : -1;
      if (at === -1) items.push(row);
      else items.splice(at, 0, row);
    }
    return { ...group, items };
  });
}

const GROUPS = withExtraItems(CORE_GROUPS, EXTRA_NAV_ITEMS);

const GROUP_OF: Record<ConsolePlace["kind"], string> = {
  account: "Account",
  organization: "Organization",
  project: "Project",
};

/** The rail for where `pathname` stands; none off the console's own paths. */
export function buildConsoleNav(pathname: string, flags: FlagSet = {}): ConsoleNavGroup[] {
  const place = consolePlace(pathname);
  if (!place) return [];
  const group = GROUPS.find((g) => g.title === GROUP_OF[place.kind]);
  if (!group) return [];

  const href = (page: string) =>
    place.kind === "account"
      ? `/${ACCOUNT_SEGMENT}${page}`
      : place.kind === "organization"
        ? organizationPath(place.organization, page)
        : projectPath(place.organization, place.project, page);
  const overview = href("");

  return [
    {
      title: group.title,
      items: group.items
        .filter((s) => !allOff(flags, s.flags ?? []))
        .map((s) => {
          const url = href(s.path);
          return {
            title: s.title,
            url,
            icon: s.icon,
            ...(s.block ? { block: s.block } : {}),
            ...(s.key ? { key: s.key } : {}),
            isActive:
              s.path === ""
                ? pathname === overview || pathname === `${overview}/`
                : pathname === url || pathname.startsWith(`${url}/`),
          };
        }),
    },
  ];
}
