/**
 * Utilities for URL slug generation and resolution across Organizations and Projects.
 *
 * Modeled after industry standards (Vercel, GitHub, Linear):
 * - Organizations and Projects have immutable internal IDs (e.g. `org_...`, `project_...`).
 * - They also carry human-friendly, URL-safe slugs (e.g. `acme`, `web-app`).
 * - Top-level reserved keywords (e.g. `api`, `auth`, `account`, `console`) are strictly prevented from colliding.
 * - Route segments accept both slugs and IDs, resolving seamlessly without breaking existing links or bookmarks.
 */

/**
 * Reserved words that cannot be used as an organization or project slug.
 * Mirrors top-level routes and standard SaaS reserved namespaces.
 */
export const RESERVED_SLUGS = new Set([
  // Core routes
  "api",
  "auth",
  "account",
  "console",
  "legal",
  "invite",
  "connect",
  "cli",
  "v1",
  "v2",
  "_next",
  "public",
  "static",
  "assets",
  "favicon.ico",
  "robots.txt",
  "sitemap.xml",
  "security.txt",
  "manifest.json",
  "root",
  "system",
  "null",
  "undefined",
  "true",
  "false",
  // Common entity and action words
  "organization",
  "organizations",
  "org",
  "project",
  "projects",
  "admin",
  "dashboard",
  "settings",
  "billing",
  "plans",
  "about",
  "support",
  "help",
  "docs",
  "documentation",
  "status",
  "health",
  "metrics",
  "internal",
  "login",
  "logout",
  "signin",
  "signout",
  "signup",
  "register",
  "pricing",
  "features",
  "team",
  "teams",
  "user",
  "users",
  "new",
  "create",
  "webhook",
  "webhooks",
  "oauth",
  "well-known",
  ".well-known",
]);

/**
 * Checks if a string is a valid slug according to standard SaaS slug rules:
 * - 2 to 48 characters
 * - lowercase alphanumeric and single hyphens
 * - does not start or end with a hyphen
 * - not a reserved keyword
 */
export function isValidSlug(slug: string): boolean {
  if (!slug || slug.length < 2 || slug.length > 48) return false;
  if (RESERVED_SLUGS.has(slug.toLowerCase())) return false;
  return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug);
}

/**
 * Converts arbitrary text into a URL-friendly, safe slug:
 * - Lowercase
 * - Strips non-alphanumeric characters (except spaces and hyphens)
 * - Collapses consecutive spaces/hyphens/underscores to a single hyphen
 * - Trims leading/trailing hyphens
 * - Limits length to 48 chars without cutting off in a dangling hyphen
 * - Guarantees the slug does not collide with reserved words by appending a qualifier
 */
export function slugify(text: string, qualifier = "app"): string {
  let slug = text
    .toLowerCase()
    .trim()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "") // strip diacritics
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/[\s_]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-+|-+$/g, "");

  if (slug.length > 48) {
    slug = slug.slice(0, 48).replace(/-+$/, "");
  }

  if (slug.length < 2) {
    return "";
  }

  if (RESERVED_SLUGS.has(slug)) {
    slug = `${slug}-${qualifier}`;
    if (slug.length > 48) {
      slug = slug.slice(0, 48).replace(/-+$/, "");
    }
  }

  return slug;
}

/**
 * Checks whether a requested URL segment matches an organization by slug, id, or slugified name.
 */
export function organizationMatches(
  org: { organizationId: string; slug?: string | null; name?: string | null },
  segment: string,
): boolean {
  if (!segment) return false;
  const seg = segment.toLowerCase();
  if (org.organizationId.toLowerCase() === seg) return true;
  if (org.slug && org.slug.toLowerCase() === seg) return true;
  if (org.name) {
    const derived = slugify(org.name, "org");
    if (derived && derived === seg) return true;
  }
  return false;
}

/**
 * Checks whether a requested URL segment matches a project by slug, id, or slugified name.
 */
export function projectMatches(
  project: { id: string; slug?: string | null; name?: string | null },
  segment: string,
): boolean {
  if (!segment) return false;
  const seg = segment.toLowerCase();
  if (project.id.toLowerCase() === seg) return true;
  if (project.slug && project.slug.toLowerCase() === seg) return true;
  if (project.name) {
    const derived = slugify(project.name, "project");
    if (derived && derived === seg) return true;
  }
  return false;
}

/**
 * Returns the preferred URL segment for an organization (slug > slugified name > organizationId).
 */
export function organizationSegment(org: {
  organizationId: string;
  slug?: string | null;
  name?: string | null;
}): string {
  if (org.slug && isValidSlug(org.slug)) return org.slug;
  if (org.name) {
    const s = slugify(org.name, "org");
    if (s.length >= 2 && isValidSlug(s)) return s;
  }
  return org.organizationId;
}

/**
 * Returns the preferred URL segment for a project (slug > slugified name > id).
 */
export function projectSegment(project: {
  id: string;
  slug?: string | null;
  name?: string | null;
}): string {
  if (project.slug && isValidSlug(project.slug)) return project.slug;
  if (project.name) {
    const s = slugify(project.name, "project");
    if (s.length >= 2 && isValidSlug(s)) return s;
  }
  return project.id;
}
