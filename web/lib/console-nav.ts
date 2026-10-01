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

export const NAV_COLLAPSED_COOKIE = "telmoni-nav-collapsed";

export function isOrganizationSegment(segment: string): boolean {
  return segment === "organization";
}

export function rootSegment(pathname: string): string {
  return pathname.split("/")[1] ?? "";
}

export const ACCOUNT_SEGMENT = "account";

export function isAccountPath(pathname: string): boolean {
  return rootSegment(pathname) === ACCOUNT_SEGMENT;
}

export const NOTIFICATIONS_HREF = "/account/notifications";

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
  segment?: string;
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
    segment: "organization",
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
    segment: "account",
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

export function buildConsoleNav(
  pathname: string,
  projectId: string,
  flags: FlagSet = {},
): ConsoleNavGroup[] {
  const isAccountMode = projectId === ACCOUNT_SEGMENT || isAccountPath(pathname);

  const isOrgMode =
    isOrganizationSegment(projectId) ||
    isOrganizationSegment(rootSegment(pathname));

  const wanted = isAccountMode
    ? "Account"
    : isOrgMode
      ? "Organization"
      : "Project";
  const activeGroup = GROUPS.find((g) => g.title === wanted);
  if (!activeGroup) return [];

  const groups = [
    {
      ...activeGroup,
      items: activeGroup.items.filter((s) => !allOff(flags, s.flags ?? [])),
    },
  ];

  return build(groups, pathname, projectId);
}

function build(
  groups: GroupSpec[],
  pathname: string,
  projectId: string,
): ConsoleNavGroup[] {
  const isCurrent = (url: string) =>
    pathname === url || pathname.startsWith(`${url}/`);

  return groups.map((group) => ({
    title: group.title,
    items: group.items.map((s) => {
      const root = group.segment ?? projectId;
      const url = `/${root}${s.path}`;
      return {
        title: s.title,
        url,
        icon: s.icon,
        isActive:
          s.path === ""
            ? pathname === `/${root}` || pathname === `/${root}/`
            : isCurrent(url),
      };
    }),
  }));
}
