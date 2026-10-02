"use client";

import { useEffect, useMemo, useSyncExternalStore } from "react";

import {
  CONSOLE_TRAIL_KEY,
  EMPTY_TRAIL,
  liveResources,
  parseTrail,
  pushPath,
  type ConsoleTrail,
} from "@/lib/console-trail";
import { projectPath } from "@/lib/slug";
import {
  useActiveOrganization,
  useOrganizations,
  useProjects,
  useProjectsElsewhere,
} from "@/lib/store";

// ⚠ **Per tab, and deliberately not `localStorage`.** Two windows of the
// console are two places you are standing; where you left the other window is
// not what this tab's back arrow means. It also should not survive the tab — a
// week-old path is a worse guess than the resource's overview.
//
// ⚠ **The trail is cached in the module, and that is a requirement rather than
// an optimisation.** `useSyncExternalStore` compares snapshots by identity and
// throws on a getter that returns a fresh value every call — parsing storage
// per render would hand back a new array each time and loop. Storage is read
// once per tab and written through here, so this variable is the live copy and
// `sessionStorage` is only where it survives a reload. It is never populated
// on the server: the effect below does not run there, and readers take
// `getServerSnapshot`, so no request can see another's trail.
let cache: ConsoleTrail | null = null;

// Nothing outside this tab can change `sessionStorage`, so our own write is the
// only thing to announce — no `storage` listener, unlike `use-corners.ts`.
let listeners: Array<() => void> = [];

function emit() {
  for (const l of listeners) l();
}

function subscribe(onChange: () => void) {
  listeners = [...listeners, onChange];
  return () => {
    listeners = listeners.filter((l) => l !== onChange);
  };
}

function load(): ConsoleTrail {
  let raw: string | null;
  try {
    raw = window.sessionStorage.getItem(CONSOLE_TRAIL_KEY);
  } catch {
    return EMPTY_TRAIL;
  }
  return parseTrail(raw);
}

export function readTrail(): ConsoleTrail {
  if (cache === null) cache = load();
  return cache;
}

export function recordPath(path: string): void {
  const trail = readTrail();
  const next = pushPath(trail, path);
  if (next === trail) return;
  // The cache moves first, so a tab with storage denied still navigates
  // correctly for as long as it lives.
  cache = next;
  try {
    window.sessionStorage.setItem(CONSOLE_TRAIL_KEY, JSON.stringify(next));
  } catch {
    // Storage disabled or full. Nothing survives a reload, which returns the
    // arrow and the selector to the overviews they pointed at before.
  }
  emit();
}

function getServerSnapshot(): ConsoleTrail {
  return EMPTY_TRAIL;
}

/**
 * This tab's trail, live. On the server there is no storage, so it is empty
 * and every caller renders its fallback — React swaps in the tab's own trail
 * after hydration, and nothing can be clicked before then. In the browser it
 * is read on the first render rather than corrected by an effect afterwards,
 * so an href is right from the frame it appears.
 */
export function useConsoleTrail(): ConsoleTrail {
  return useSyncExternalStore(subscribe, readTrail, getServerSnapshot);
}

/**
 * Records where you are standing. The write is an effect because it is a
 * consequence of having navigated, not part of drawing anything.
 *
 * ⚠ **One caller: `ConsoleTrailRecorder`, mounted by the console layout.** It
 * is a layout concern — every console page is a place you can be — and a
 * second caller would write the same path twice per navigation.
 */
export function useRecordConsolePath(pathname: string): void {
  useEffect(() => {
    recordPath(pathname);
  }, [pathname]);
}

/** Test seam: the module-level cache outlives a `window.sessionStorage.clear()`. */
export function clearTrailForTest(): void {
  cache = null;
}

/**
 * What the trail is read against: the resources the console is drawing, and
 * where to go when none of its entries is one of them — the first project of
 * the organization it stands in, else `/console`, which finds somewhere.
 *
 * One hook for the rail's back arrow and the resource selector, so the two
 * walk the trail over the same list.
 */
export function useLiveResources(): { live: ReadonlySet<string>; fallback: string } {
  const organizations = useOrganizations();
  const active = useActiveOrganization();
  const projects = useProjects();
  const elsewhere = useProjectsElsewhere();
  return useMemo(() => {
    const here = active
      ? projects.map((p) => ({ slug: p.slug, organizationSlug: active.slug }))
      : [];
    const [first] = here;
    return {
      live: liveResources(organizations, [...here, ...elsewhere]),
      fallback: first ? projectPath(first.organizationSlug, first.slug) : "/console",
    };
  }, [organizations, active, projects, elsewhere]);
}
