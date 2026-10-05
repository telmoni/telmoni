/**
 * The console's paths, and the slugs that spell them:
 *
 *   /{organization}                   the organization's overview
 *   /{organization}/{page}            the organization's own pages
 *   /{organization}/{project}         a project's overview
 *   /{organization}/{project}/{page}  a project's pages
 *
 * An organization's pages sit beside its projects, so each goes by a word no
 * project may (`RESERVED_PROJECT_SLUGS`): that is how a path's second segment
 * tells the two apart, and a page a console built on this one adds there needs
 * its word on auth's list first.
 *
 * ⚠ **Auth mints every slug; nothing here derives one.** A project's slug
 * follows its name and an organization's is a setting of its own, so either
 * can move: build every path from the slug the server last answered, never
 * from a name. Only the cookie and the headers name a row by its id; a link
 * kept — the trail, a notice, a citation — is spelled with slugs and dies when
 * one moves, and the layouts redirect an id in a path to its slug.
 */

/** The longest a slug may be: `telmoni_shared::slug::MAX_LEN`. */
export const SLUG_MAX_LENGTH = 48;

/**
 * Words no organization goes by: the console's own top-level paths, the ones
 * a console built on it serves or may yet, and the ones Next answers itself.
 * Auth's list (`telmoni_shared::slug::ORGANIZATION_RESERVED`), pinned by
 * `contract.test.ts`.
 */
export const RESERVED_ORGANIZATION_SLUGS: ReadonlySet<string> = new Set([
  "404",
  "500",
  "about",
  "account",
  "accounts",
  "admin",
  "api",
  "app",
  "apple-icon",
  "apps",
  "assets",
  "auth",
  "billing",
  "blog",
  "blueprints",
  "careers",
  "changelog",
  "cli",
  "community",
  "connect",
  "console",
  "contact",
  "cookbook",
  "dashboard",
  "developers",
  "device",
  "docs",
  "download",
  "enterprise",
  "favicon",
  "health",
  "help",
  "home",
  "icon",
  "index",
  "install",
  "internal",
  "invite",
  "invites",
  "legal",
  "login",
  "logout",
  "manifest",
  "metrics",
  "new",
  "null",
  "oauth",
  "opengraph-image",
  "org",
  "organization",
  "organizations",
  "plans",
  "pricing",
  "privacy",
  "project",
  "projects",
  "robots",
  "security",
  "settings",
  "sign-in",
  "sign-out",
  "sign-up",
  "signin",
  "signout",
  "signup",
  "sitemap",
  "sso",
  "static",
  "status",
  "support",
  "team",
  "teams",
  "terms",
  "twitter-image",
  "undefined",
  "user",
  "users",
  "v1",
  "v2",
  "www",
]);

/**
 * Words no project goes by: its organization's own pages, which sit beside its
 * projects, and the ones a console built on this one serves there or may yet.
 * Auth's list (`telmoni_shared::slug::PROJECT_RESERVED`), pinned by
 * `contract.test.ts`.
 */
export const RESERVED_PROJECT_SLUGS: ReadonlySet<string> = new Set([
  "activity",
  "api-keys",
  "audit-log",
  "billing",
  "connectors",
  "integrations",
  "members",
  "new",
  "notifications",
  "plans",
  "projects",
  "security",
  "settings",
  "sso",
  "support",
  "usage",
]);

const SLUG = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** Whether `segment` has a slug's shape: lowercase words joined by single hyphens. */
export function isSlug(segment: string): boolean {
  return segment.length <= SLUG_MAX_LENGTH && SLUG.test(segment);
}

/**
 * Whether a path's first segment can name an organization: a slug that none
 * of the console's own paths claims.
 */
export function isOrganizationSlug(segment: string): boolean {
  return isSlug(segment) && !RESERVED_ORGANIZATION_SLUGS.has(segment);
}

/**
 * Whether a path's second segment is one of its organization's own pages
 * rather than a project: a word no project goes by.
 */
export function isOrganizationPage(segment: string): boolean {
  return RESERVED_PROJECT_SLUGS.has(segment);
}

/** `/{organization}`, or one of its own pages: `("acme", "/settings")` is `/acme/settings`. */
export function organizationPath(organization: string, page = ""): string {
  return `/${organization}${page}`;
}

/** `/{organization}/{project}`, or one of the project's pages. */
export function projectPath(organization: string, project: string, page = ""): string {
  return `/${organization}/${project}${page}`;
}

/**
 * `path` with its leading segments replaced by `segments`, the rest and the
 * query kept: where a layout redirects a path spelled with an id, or a stray
 * capital, to the slugs the rows go by.
 */
export function withLeadingSegments(path: string, segments: readonly string[]): string {
  const at = path.search(/[?#]/);
  const pathname = at === -1 ? path : path.slice(0, at);
  const rest = pathname.split("/").slice(1 + segments.length);
  return `/${[...segments, ...rest].join("/")}${at === -1 ? "" : path.slice(at)}`;
}
