import { z } from "zod";

import { type ConsolePlace, consolePlace } from "@/lib/console-nav";
import { organizationPath, projectPath } from "@/lib/slug";

/**
 * Where the console sends you back to.
 *
 * A trail of the last page you stood on in each resource — a project, or the
 * organization — newest first. The rail's back arrow reads the head of it to
 * leave Account for the page you came from; the resource selector reads the
 * entry for each of its rows so switching away and back does not cost you the
 * page you were reading. Not a browser history: moving around inside a
 * resource replaces its entry rather than stacking on it.
 *
 * Pure on purpose. The store that holds a tab's trail, and the hooks that read
 * it, are in `use-console-trail.ts` — the same split as `corners.ts` and
 * `use-corners.ts`, so that a server component can import these without
 * dragging in a `"use client"` boundary it cannot call through.
 */

export const CONSOLE_TRAIL_KEY = "telmoni-console-trail";

// One entry per resource, so this caps a number of projects and not a depth of
// history: four projects and the organization is five. Nobody switches between
// more than a handful, and the oldest falling off costs an overview.
export const TRAIL_LIMIT = 8;

/**
 * A path that has been through `isReturnablePath`. The type says less than the
 * guard checks — only that it is absolute — but it is enough to keep a bare
 * string out of the trail at compile time, so every entry went past the guard.
 */
export type ReturnablePath = `/${string}`;

export type ConsoleTrail = readonly ReturnablePath[];

/** One shared empty trail: the server snapshot and every failed parse hand
 *  this back, so an absent trail compares equal to itself across renders. */
export const EMPTY_TRAIL: ConsoleTrail = [];

const TrailSchema = z.array(z.string());

/**
 * A path the console may be sent back to: a page of an organization or of one
 * of its projects. Everything else is a refusal:
 *
 * - Anything that is not an absolute in-app path, because these end up in an
 *   `href` and storage is writable by anything else on the origin. `//` is
 *   protocol-relative and leaves the site; a backslash is how some parsers read
 *   the same thing.
 * - An account path. Account is the mode you step INTO and leave by the back
 *   arrow, so recording one would make the arrow a no-op — and it is not a
 *   resource the selector can switch to either.
 * - `/console`, which is a door and not a destination: it asks the owner to name an unnamed
 *   organization, or resolves to the first project or the Projects page, so
 *   remembering it would land you somewhere you never stood.
 */
export function isReturnablePath(path: string): path is ReturnablePath {
  if (!path.startsWith("/") || path.startsWith("//")) return false;
  if (path.includes("\\")) return false;
  const place = consolePlace(path);
  return place !== null && place.kind !== "account";
}

/** A resource the console stands in: an organization, or one of its projects. */
export type Resource = Exclude<ConsolePlace, { kind: "account" }>;

/** The resource's overview, which is also what the trail keys it by. */
export function resourceRoot(resource: Resource): ReturnablePath {
  return (
    resource.kind === "organization"
      ? organizationPath(resource.organization)
      : projectPath(resource.organization, resource.project)
  ) as ReturnablePath;
}

function resourceOf(path: string): Resource | null {
  const place = consolePlace(path);
  return place === null || place.kind === "account" ? null : place;
}

/** The resource `path` is a page of, as its root; `""` for a path of none. */
export function trailResource(path: string): string {
  const resource = resourceOf(path);
  return resource ? resourceRoot(resource) : "";
}

/**
 * The trail after arriving at `path`. Same reference back when nothing would
 * change — the path is refused, or it is already the head — and that is a
 * contract, not thrift: the store compares snapshots by identity, and a fresh
 * array for an unchanged trail re-renders every reader for nothing.
 */
export function pushPath(trail: ConsoleTrail, path: string): ConsoleTrail {
  if (!isReturnablePath(path) || trail[0] === path) return trail;
  const resource = trailResource(path);
  return [path, ...trail.filter((p) => trailResource(p) !== resource)].slice(
    0,
    TRAIL_LIMIT,
  );
}

/**
 * A stored trail back into shape. What is in storage was not necessarily put
 * there by this build, or by us at all, so nothing is trusted: a value that is
 * not a list of strings is an empty trail, and within one, an entry the guard
 * would refuse is dropped, a second entry for a resource is dropped, and the
 * cap is applied again.
 */
export function parseTrail(raw: string | null): ConsoleTrail {
  if (raw === null) return EMPTY_TRAIL;
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    return EMPTY_TRAIL;
  }
  const parsed = TrailSchema.safeParse(body);
  if (!parsed.success) return EMPTY_TRAIL;
  const seen = new Set<string>();
  const trail: ReturnablePath[] = [];
  for (const path of parsed.data) {
    if (!isReturnablePath(path)) continue;
    const resource = trailResource(path);
    if (seen.has(resource)) continue;
    seen.add(resource);
    trail.push(path);
    if (trail.length === TRAIL_LIMIT) break;
  }
  return trail.length === 0 ? EMPTY_TRAIL : trail;
}

/**
 * The roots of every resource the console is drawing: each organization the
 * person is in, and each project they can open, in the organization the
 * console stands in and in the others.
 */
export function liveResources(
  organizations: readonly { slug: string }[],
  projects: readonly { slug: string; organizationSlug: string }[],
): ReadonlySet<string> {
  return new Set([
    ...organizations.map((o) => organizationPath(o.slug)),
    ...projects.map((p) => projectPath(p.organizationSlug, p.slug)),
  ]);
}

/**
 * Where the rail's back arrow points: the last page you stood on before
 * stepping into Account.
 *
 * ⚠ **Each entry is checked against the resources the console is drawing, not
 * trusted.** You can leave a project, or lose your role in it, from inside
 * Account — its connectors page is then a 404 with a back arrow aimed at it —
 * and a moved slug — a project renamed, an organization's URL changed — leaves
 * the path that was recorded naming nothing. The walk carries on to the next
 * resource rather than giving up, so somebody who left one project still gets
 * the page they were on in another.
 *
 * Only the resource is checked, never the role a page needs: the rail does not
 * know which rows a project grants, and a page you can reach but not read
 * already renders its own refusal.
 */
export function resolveReturnUrl(
  trail: ConsoleTrail,
  live: ReadonlySet<string>,
  fallback: string,
): string {
  return trail.find((p) => live.has(trailResource(p))) ?? fallback;
}

/**
 * Which resource the console's chrome should NAME, as opposed to which page it
 * is on.
 *
 * Every path but Account's names its own resource. Account names none — it
 * is a mode you step INTO, not a resource you switch to — so chrome that has
 * to keep naming one reads the resource you stepped in FROM, by the same walk
 * the rail's back arrow takes, and `fallback` when the trail has none.
 *
 * ⚠ **Both callers go through here so they cannot name two different places.**
 * The selector said "Project" the moment you opened Account, having nothing in
 * the URL to read, while the arrow beside it pointed at the project you came
 * from — one piece of chrome claiming you had left and another claiming you
 * had not.
 */
export function standingResource(
  pathname: string,
  trail: ConsoleTrail,
  live: ReadonlySet<string>,
  fallback: string,
): Resource | null {
  const place = consolePlace(pathname);
  if (place !== null && place.kind !== "account") return place;
  return resourceOf(resolveReturnUrl(trail, live, fallback));
}

/**
 * Where a row in the resource selector points: the page you were last on in
 * that resource, or its overview the first time you open it.
 *
 * The resource is one the selector is already drawing, so it needs no liveness
 * check — the list IS the check.
 */
export function resourceUrl(trail: ConsoleTrail, resource: Resource): string {
  const root = resourceRoot(resource);
  return trail.find((p) => trailResource(p) === root) ?? root;
}
