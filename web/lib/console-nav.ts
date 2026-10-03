import {
  Activity,
  Bell,
  History,
  KeyRound,
  Plug,
  Settings,
  ShieldCheck,
  SquareStack,
  User,
  type LucideIcon,
  Users,
} from "lucide-react";

import { EXTRA_NAV_ITEMS, type ExtraNavItem } from "@/lib/extension/nav";
import { Flag, type FlagSet, allOff } from "@/lib/flags";
import {
  ORGANIZATION_PAGES,
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
  if (second === "" || second === ORGANIZATION_PAGES) {
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
}

export interface ConsoleNavGroup {
  title: string;
  items: ConsoleNavItem[];
}

interface GroupSpec {
  title: string;
  items: {
    title: string;
    path: string;
    icon: LucideIcon;
    flags?: readonly Flag[];
  }[];
}

const CORE_GROUPS: GroupSpec[] = [
  {
    title: "Project",
    items: [
      { title: "Overview", path: "", icon: Activity },
      { title: "API keys", path: "/api-keys", icon: KeyRound },
      { title: "Connectors", path: "/connectors", icon: Plug },
      { title: "Members", path: "/members", icon: Users },
      { title: "Audit log", path: "/audit-log", icon: History },
      { title: "Settings", path: "/settings", icon: Settings },
    ],
  },
  {
    title: "Organization",
    items: [
      { title: "Overview", path: "", icon: Activity },
      { title: "Projects", path: "/projects", icon: SquareStack },
      { title: "Members", path: "/members", icon: Users },
      { title: "Audit log", path: "/audit-log", icon: History },
      { title: "Settings", path: "/settings", icon: Settings },
    ],
  },
  {
    title: "Account",
    items: [
      { title: "Settings", path: "/settings", icon: User },
      { title: "Notifications", path: "/notifications", icon: Bell },
      { title: "Privacy", path: "/privacy", icon: ShieldCheck },
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
      const row = { title: extra.title, path: extra.path, icon: extra.icon };
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
            isActive:
              s.path === ""
                ? pathname === overview || pathname === `${overview}/`
                : pathname === url || pathname.startsWith(`${url}/`),
          };
        }),
    },
  ];
}
