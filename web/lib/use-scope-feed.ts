"use client";

import { usePathname } from "next/navigation";
import { useCallback, useEffect, useRef, useState } from "react";

import { readFeedAction, type FeedScope } from "@/app/(app)/feed-actions";
import { standingResource } from "@/lib/console-trail";
import { administers } from "@/lib/roles";
import type { NotificationFeed } from "@/lib/server/data";
import { useActiveOrganization, useProjects } from "@/lib/store";
import { useConsoleTrail, useLiveResources } from "@/lib/use-console-trail";

// The bell's feeds: those of the place the console stands in, as the header's
// switcher names it — a project's, and the organization's own for its owners
// and admins, who alone may read it. Each read as the menu opens, a preview
// of the feed's own page.

/** A feed the bell reads, with what its rows link by. */
export type BellScope =
  | { kind: "project"; projectId: string; organization: string; project: string; name: string }
  | { kind: "organization"; organizationId: string; organization: string; name: string };

/** The feeds where the console stands that the person may read: the
 *  project's, on a project's pages, and the organization's own for its owners
 *  and admins, on any of its pages; none off an organization's. */
export function useBellPlaces(): {
  project: Extract<BellScope, { kind: "project" }> | null;
  organization: Extract<BellScope, { kind: "organization" }> | null;
} {
  const trail = useConsoleTrail();
  const { live, fallback } = useLiveResources();
  const standing = standingResource(usePathname(), trail, live, fallback);
  const organization = useActiveOrganization();
  const projects = useProjects();
  if (!organization || standing?.organization !== organization.slug) {
    return { project: null, organization: null };
  }
  const project = standing.kind === "project" ? projects.find((p) => p.slug === standing.project) : undefined;
  return {
    project: project
      ? {
          kind: "project",
          projectId: project.id,
          organization: organization.slug,
          project: project.slug,
          name: project.name,
        }
      : null,
    organization: administers(organization.role)
      ? {
          kind: "organization",
          organizationId: organization.organizationId,
          organization: organization.slug,
          name: organization.name,
        }
      : null,
  };
}

/** The scope as the action takes it. */
function asked(scope: BellScope): FeedScope {
  return scope.kind === "project"
    ? { kind: "project", projectId: scope.projectId }
    : { kind: "organization", organizationId: scope.organizationId };
}

/** `scope`'s feed, none until read or where it cannot be. A read for a
 *  place left since is dropped. */
export function useScopeFeed(scope: BellScope | null, open: boolean) {
  const [feed, setFeed] = useState<{ key: string; feed: NotificationFeed | null } | null>(null);
  const key = scope ? JSON.stringify(asked(scope)) : null;
  const latest = useRef(key);
  // First of the effects, so a read the place's change starts already knows it.
  useEffect(() => {
    latest.current = key;
  }, [key]);

  const reload = useCallback(async () => {
    if (!key) return;
    const read = await readFeedAction(JSON.parse(key));
    if (latest.current === key) setFeed({ key, feed: read });
  }, [key]);

  // The badge counts none of a feed's notices, so a feed is read only for
  // the menu that shows it: as it opens, and as the place changes under it.
  useEffect(() => {
    if (open) void reload();
  }, [open, reload]);

  return { feed: feed && feed.key === key ? feed.feed : null, read: feed?.key === key };
}
