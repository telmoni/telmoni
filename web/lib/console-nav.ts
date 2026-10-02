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

export const ACCOUNT_SEGMENT = "account";

export const NOTIFICATIONS_HREF = "/account/notifications";

const PROJECT_ACTIONS = new Set(["api-keys", "connectors"]);
const COMMON_ACTIONS = new Set(["members", "audit-log", "settings"]);
const ORG_ACTIONS = new Set(["projects", "billing"]);

const ORG_SUBPAGES = new Set([
  "",
  ...ORG_ACTIONS,
  ...COMMON_ACTIONS,
]);

export function isOrganizationSegment(segment: string): boolean {
  return segment === "organization" || segment.startsWith("org_") || segment === "org";
}

export function rootSegment(pathname: string): string {
  return pathname.split("/")[1] ?? "";
}

export function isAccountPath(pathname: string): boolean {
  return rootSegment(pathname) === ACCOUNT_SEGMENT;
}

export interface ParsedConsolePath {
  organizationId: string | null;
  orgId: string | null;
  projectId: string | null;
  mode: "account" | "organization" | "project" | "unknown";
}

export function parseConsolePath(
  pathname: string,
  explicitSegment?: string,
): ParsedConsolePath {
  const parts = pathname.split("/").filter(Boolean);
  if (parts.length === 0) {
    return { organizationId: null, orgId: null, projectId: null, mode: "unknown" };
  }
  if (parts[0] === ACCOUNT_SEGMENT || explicitSegment === ACCOUNT_SEGMENT) {
    return { organizationId: null, orgId: null, projectId: null, mode: "account" };
  }
  if (parts[0] === "console" || parts[0] === "invite" || parts[0] === "auth") {
    return { organizationId: null, orgId: null, projectId: null, mode: "unknown" };
  }

  // If 3+ segments, e.g. /org-1/prj-123/connectors OR /prj-123/api-keys/abc:
  if (parts.length >= 3) {
    if (PROJECT_ACTIONS.has(parts[1]) || COMMON_ACTIONS.has(parts[1])) {
      return { organizationId: null, orgId: null, projectId: parts[0], mode: "project" };
    }
    return { organizationId: parts[0], orgId: parts[0], projectId: parts[1], mode: "project" };
  }

  // If 2 segments:
  if (parts.length === 2) {
    if (parts[0] === "organization") {
      return { organizationId: "organization", orgId: "organization", projectId: null, mode: "organization" };
    }
    if (explicitSegment && explicitSegment === parts[0] && explicitSegment !== "organization") {
      return { organizationId: null, orgId: null, projectId: parts[0], mode: "project" };
    }
    if (PROJECT_ACTIONS.has(parts[1])) {
      return { organizationId: null, orgId: null, projectId: parts[0], mode: "project" };
    }
    if (ORG_SUBPAGES.has(parts[1])) {
      return { organizationId: parts[0], orgId: parts[0], projectId: null, mode: "organization" };
    }
    // E.g. /org-1/prj-123 (project overview under org)
    return { organizationId: parts[0], orgId: parts[0], projectId: parts[1], mode: "project" };
  }

  // 1 segment: e.g. /organization or /org-1 or /prj-123
  if (explicitSegment && explicitSegment !== "organization" && explicitSegment !== parts[0]) {
    return { organizationId: parts[0], orgId: parts[0], projectId: explicitSegment, mode: "project" };
  }
  return { organizationId: parts[0], orgId: parts[0], projectId: null, mode: "organization" };
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
  projectIdOrSegment?: string,
  flags: FlagSet = {},
): ConsoleNavGroup[] {
  const parsed = parseConsolePath(pathname, projectIdOrSegment);

  const wanted =
    parsed.mode === "account"
      ? "Account"
      : parsed.mode === "organization"
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

  return build(groups, pathname, parsed);
}

function build(
  groups: GroupSpec[],
  pathname: string,
  parsed: ParsedConsolePath,
): ConsoleNavGroup[] {
  const isCurrent = (url: string) =>
    pathname === url || pathname.startsWith(`${url}/`);

  return groups.map((group) => ({
    title: group.title,
    items: group.items.map((s) => {
      let root = "";
      if (group.title === "Account") {
        root = "account";
      } else if (group.title === "Organization") {
        root = parsed.organizationId ?? parsed.orgId ?? "organization";
      } else {
        const org = parsed.organizationId ?? parsed.orgId;
        if (org && parsed.projectId) {
          root = `${org}/${parsed.projectId}`;
        } else {
          root = parsed.projectId ?? org ?? "";
        }
      }
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
