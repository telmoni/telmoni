/**
 * The console's paths, and the slugs that spell them:
 *
 *   /{organization}                   the organization's overview
 *   /{organization}/~/{page}          the organization's own pages
 *   /{organization}/{project}         a project's overview
 *   /{organization}/{project}/{page}  a project's pages
 *
 * `~` is no slug's shape, so a project can go by any slug at all without
 * landing on one of its organization's pages, and a console built on this one
 * can add an organization page without reserving its name.
 *
 * ⚠ **Auth mints every slug; nothing here derives one.** A slug follows its
 * row's name, so a rename moves the URL: build every path from the slug the
 * server last answered, never from a name. Something kept longer than a page
 * names the row by its id, and the layouts redirect an id to its slug.
 */

/** The segment an organization's own pages sit under. */
export const ORGANIZATION_PAGES = "~";

/** The longest a slug may be: `telmoni_shared::slug::MAX_LEN`. */
export const SLUG_MAX_LENGTH = 48;

/**
 * Words no organization goes by: the console's own top-level paths, the ones
 * a console built on it serves or may yet, and the ones Next answers itself.
 * Auth's list (`telmoni_shared::slug::RESERVED`), pinned by `contract.test.ts`.
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

/** `/{organization}`, or one of its own pages: `("acme", "/settings")` is `/acme/~/settings`. */
export function organizationPath(organization: string, page = ""): string {
  return page ? `/${organization}/${ORGANIZATION_PAGES}${page}` : `/${organization}`;
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
