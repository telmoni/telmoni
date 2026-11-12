import {
  Accessibility,
  Bell,
  BellRing,
  Bot,
  Contact,
  Gauge,
  History,
  KeyRound,
  ListTree,
  MessagesSquare,
  Plug,
  Settings,
  ShieldCheck,
  SquareStack,
  Target,
  User,
  type LucideIcon,
  Users,
  Wallet,
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

/** The path of `url` where it is one this console can route, else none: an
 *  address the server names on this console's own origin. */
export function pathOf(url: string): string | null {
  if (url.startsWith("/") && !url.startsWith("//")) return url;
  try {
    const { pathname } = new URL(url);
    return pathname.startsWith("/") ? pathname : null;
  } catch {
    return null;
  }
}

export const NAV_COLLAPSED_COOKIE = "telmoni-nav-collapsed";

export const ACCOUNT_SEGMENT = "settings";

export const NOTIFICATIONS_HREF = "/settings/notifications";

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

export interface ConsoleNavItem {
  title: string;
  url: string;
  icon: LucideIcon;
  isActive: boolean;
  /** Opens a block: this row and the rest up to the next that opens one stand
   *  clear of the rows above. A group's first row opens its first block. */
  block?: boolean;
  /** The letter after `g` that reaches the row (`lib/keys.ts`). */
  key?: string;
}

export interface ConsoleNavGroup {
  title: string;
  items: ConsoleNavItem[];
}

/** A group's rows in their blocks, for the rail to draw. */
export function navBlocks(items: readonly ConsoleNavItem[]): ConsoleNavItem[][] {
  const blocks: ConsoleNavItem[][] = [];
  for (const item of items) {
    if (item.block || blocks.length === 0) blocks.push([]);
    blocks[blocks.length - 1]?.push(item);
  }
  return blocks;
}

interface GroupSpec {
  title: string;
  items: {
    title: string;
    path: string;
    /** A path the row's pages also stand under, where its own is the rail's
     *  root: an agent's page, under the Agents row. */
    under?: string;
    icon: LucideIcon;
    flags?: readonly Flag[];
    block?: boolean;
    key?: string;
  }[];
}

// `key` is the letter after `g` that reaches a row, so the `g` sequences are
// the rail itself and a row added here is reached the day it lands. Single,
// unique within a rail (two rails may share one: `p` is Projects on an
// organization's and Privacy on a project's and on Settings'), never `g`, `?` or shift-`O`, the
// one sequence that crosses rails (`lib/keys.ts`). `,` reaches Settings on
// every rail, as `⌘,` opens preferences on a Mac; `c` is Connectors and also
// the page's action, which `lib/keys.ts` explains is no collision.

const CORE_GROUPS: GroupSpec[] = [
  {
    title: "Project",
    // What answers "are my agents OK?", then the project's own housekeeping a
    // block below. The pages keep their paths: Spend is the dashboards.
    items: [
      { title: "Agents", path: "", under: "/agents", icon: Bot, key: "o" },
      { title: "Traces", path: "/traces", icon: ListTree, key: "t" },
      { title: "Sessions", path: "/sessions", icon: MessagesSquare, key: "e" },
      // End users, the people the agents worked for: Members, below, are the project's own.
      { title: "Users", path: "/users", icon: Contact, key: "u" },
      { title: "Alerts", path: "/alerts", icon: BellRing, key: "a" },
      { title: "Objectives", path: "/objectives", icon: Target, key: "v" },
      { title: "Spend", path: "/dashboards", icon: Wallet, key: "s" },
      { title: "API keys", path: "/api-keys", icon: KeyRound, block: true, key: "k" },
      { title: "Connectors", path: "/connectors", icon: Plug, key: "c" },
      { title: "Members", path: "/members", icon: Users, key: "m" },
      { title: "Audit log", path: "/audit-log", icon: History, key: "l" },
      { title: "Notifications", path: "/notifications", icon: Bell, key: "n" },
      { title: "Settings", path: "/settings", icon: Settings, key: "," },
    ],
  },
  {
    title: "Organization",
    // An organization opens on its projects (`[organization]/page.tsx`).
    items: [
      { title: "Projects", path: "/projects", icon: SquareStack, key: "p" },
      { title: "Members", path: "/members", icon: Users, key: "m" },
      { title: "Audit log", path: "/audit-log", icon: History, key: "l" },
      { title: "Notifications", path: "/notifications", icon: Bell, key: "n" },
      { title: "Limits", path: "/limits", icon: Gauge, key: "i" },
      { title: "Settings", path: "/settings", icon: Settings, key: "," },
    ],
  },
  {
    title: "Settings",
    // One block: the way back above it says what this rail is.
    items: [
      { title: "General", path: "/general", icon: Settings, key: "," },
      { title: "Account", path: "/account", icon: User, key: "u" },
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
  account: "Settings",
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
          const at = (page: string) => pathname === page || pathname.startsWith(`${page}/`);
          return {
            title: s.title,
            url,
            icon: s.icon,
            ...(s.block ? { block: s.block } : {}),
            ...(s.key ? { key: s.key } : {}),
            isActive:
              s.path === ""
                ? pathname === overview ||
                  pathname === `${overview}/` ||
                  (s.under !== undefined && at(href(s.under)))
                : at(url),
          };
        }),
    },
  ];
}
