// Which organization a request acts in. The path decides (`lib/slug.ts`): the
// proxy reads it there and hands it on, so a page and every action posted from
// it act in the organization the page shows. Off an organization's path a
// cookie stands in. Both are requests, never claims: auth's `/me` answers with
// an organization only when the person is in it.

import { ORGANIZATION_PAGES, isOrganizationSlug } from "@/lib/slug";

/**
 * The organization the path names, by slug. Set by the proxy and nowhere
 * else — it drops a client's own copy — on a prefetch as on any other
 * request, so what is fetched ahead is rendered for the organization its path
 * names. Absent off an organization's path, where the cookie below stands in.
 */
export const ORGANIZATION_HEADER = "x-telmoni-organization";

/**
 * The path and query, for the layouts that redirect an id or a stray capital
 * to the canonical slug: a layout is never handed the path itself. Set by the
 * proxy on every request.
 */
export const PATH_HEADER = "x-telmoni-path";

/**
 * The organization the console last stood in, for the paths that name none:
 * Account, `/console`, the route handlers. By id, not by slug: it outlives
 * the page that wrote it, and a rename moves a slug.
 *
 * ⚠ **Written by the console in the browser** (`OrganizationSync`), **never
 * by the proxy.** Only a page that is on screen may move it, and the proxy
 * cannot tell one from a prefetch: Next strips the headers that mark a
 * prefetch before the proxy runs, and the router prefetches every link it
 * draws — the resource selector's point into every organization. Hence no
 * `HttpOnly`, which costs nothing: the cookie claims nothing, and auth honours
 * it only for an organization the person is in.
 */
export const ACTIVE_ORGANIZATION_COOKIE = "telmoni-organization";

const REMEMBERED_SECONDS = 60 * 60 * 24 * 30;

/** What a server-side write carries: the same attributes as the browser's. */
export const ACTIVE_ORGANIZATION_COOKIE_OPTIONS = {
  path: "/",
  secure: process.env.NODE_ENV === "production",
  sameSite: "lax",
  maxAge: REMEMBERED_SECONDS,
} as const;

/** The cookie as `document.cookie` takes it. */
export function activeOrganizationCookie(organizationId: string, secure: boolean): string {
  return [
    `${ACTIVE_ORGANIZATION_COOKIE}=${organizationId}`,
    "Path=/",
    `Max-Age=${REMEMBERED_SECONDS}`,
    "SameSite=Lax",
    ...(secure ? ["Secure"] : []),
  ].join("; ");
}

/**
 * `pathname` with the segment an organization's own pages sit under spelled
 * plainly, when something escaped it; `null` when there is nothing to put
 * right. `~` needs no escaping in a path, and a chat client, a mail client or
 * a link checker may write `%7E` all the same, into a link a notice carried.
 * The router matches the literal, and would read the escaped one as a
 * project's name.
 */
export function unescapedPath(pathname: string): string | null {
  const unescaped = pathname.replace(/^(\/[^/]+)\/%7e(?=\/|$)/i, `$1/${ORGANIZATION_PAGES}`);
  return unescaped === pathname ? null : unescaped;
}

/**
 * The organization `pathname` names: its first segment, when that is a slug
 * none of the console's own paths claims. `null` for those, and for an id,
 * which the layouts redirect to its slug before anything acts on it.
 */
export function namedOrganization(pathname: string): string | null {
  const segment = pathname.split("/")[1] ?? "";
  return isOrganizationSlug(segment) ? segment : null;
}
