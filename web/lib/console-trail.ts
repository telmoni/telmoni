import { z } from "zod";

import {
  ACCOUNT_SEGMENT,
  isAccountPath,
  isOrganizationSegment,
  rootSegment,
} from "@/lib/console-nav";

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
 * A path the console may be sent back to. Everything here is a refusal:
 *
 * - Anything that is not an absolute in-app path, because these end up in an
 *   `href` and storage is writable by anything else on the origin. `//` is
 *   protocol-relative and leaves the site; a backslash is how some parsers read
 *   the same thing.
 * - An account path. Account is the mode you step INTO and leave by the back
 *   arrow, so recording one would make the arrow a no-op — and it is not a
 *   resource the selector can switch to either.
 * - `/console`, which is a redirect and not a destination: it resolves to the
 *   first project, so remembering it would land you somewhere you never stood.
 */
export function isReturnablePath(path: string): path is ReturnablePath {
  if (!path.startsWith("/") || path.startsWith("//")) return false;
  if (path.includes("\\")) return false;
  if (isAccountPath(path)) return false;
  return path !== "/console";
}

/**
 * The trail after arriving at `path`. Same reference back when nothing would
 * change — the path is refused, or it is already the head — and that is a
 * contract, not thrift: the store compares snapshots by identity, and a fresh
 * array for an unchanged trail re-renders every reader for nothing.
 */
export function pushPath(trail: ConsoleTrail, path: string): ConsoleTrail {
  if (!isReturnablePath(path) || trail[0] === path) return trail;
  const resource = rootSegment(path);
  return [path, ...trail.filter((p) => rootSegment(p) !== resource)].slice(
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
    const resource = rootSegment(path);
    if (seen.has(resource)) continue;
    seen.add(resource);
    trail.push(path);
    if (trail.length === TRAIL_LIMIT) break;
  }
  return trail.length === 0 ? EMPTY_TRAIL : trail;
}

function isLiveResource(resource: string, projectIds: readonly string[]): boolean {
  return isOrganizationSegment(resource) || projectIds.includes(resource);
}

/**
 * Where the rail's back arrow points: the last page you stood on before
 * stepping into Account.
 *
 * ⚠ **Each entry is checked against the rail's own project list, not trusted.**
 * You can leave a project, or lose your role in it, from inside Account —
 * `/{project}/connectors` is then a 404 with a back arrow aimed at it. The walk
 * carries on to the next resource rather than giving up, so somebody who left
 * one project still gets the page they were on in another.
 *
 * Only the project is checked, never the role a page needs: the rail does not know
 * which rows a project grants, and a page you can reach but not read already
 * renders its own refusal.
 */
export function resolveReturnUrl(
  trail: ConsoleTrail,
  projectIds: readonly string[],
  fallback: string,
): string {
  return (
    trail.find((p) => isLiveResource(rootSegment(p), projectIds)) ?? fallback
  );
}

/**
 * Which resource the console's chrome should NAME, as opposed to which page it
 * is on.
 *
 * Every segment but `account` names its own resource. Account names none — it
 * is a mode you step INTO, not a resource you switch to — so chrome that has
 * to keep naming one reads the resource you stepped in FROM, by the same walk
 * the rail's back arrow takes.
 *
 * ⚠ **Both callers go through here so they cannot name two different places.**
 * The selector said "Project" the moment you opened Account, having nothing in
 * the URL to read, while the arrow beside it pointed at the project you came
 * from — one piece of chrome claiming you had left and another claiming you
 * had not.
 */
export function standingResource(
  segment: string,
  trail: ConsoleTrail,
  projectIds: readonly string[],
): string {
  if (segment !== ACCOUNT_SEGMENT) return segment;
  const first = projectIds[0];
  return rootSegment(resolveReturnUrl(trail, projectIds, first ? `/${first}` : ""));
}

/**
 * Where a row in the resource selector points: the page you were last on in
 * that resource, or its overview the first time you open it.
 *
 * The resource is one the selector is already drawing, so it needs no liveness
 * check — the list IS the check.
 */
export function resourceUrl(trail: ConsoleTrail, resource: string): string {
  return trail.find((p) => rootSegment(p) === resource) ?? `/${resource}`;
}
