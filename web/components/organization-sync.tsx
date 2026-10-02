"use client";

import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";

import { organizationToRemember, staleShellOrganization } from "@/lib/console-nav";
import { activeOrganizationCookie } from "@/lib/proxy/organization";
import { useActiveOrganizationId, useOrganizations } from "@/lib/store";

// What follows the path from one organization into another, and cannot be
// done on the server.
//
// The cookie, for the paths that name no organization. Written here, on every
// page that is on screen, because only that is somebody opening one: the
// proxy also sees every prefetch, and cannot tell them apart.
//
// The `(app)` layout, which is shared by every organization's pages: the
// router keeps it, and everything it rendered for one organization, across a
// move into another. `rendered` is the slug of the organization it was
// rendered for, and when the path names a different one the layout is asked
// for again. Once per arrival, so an answer that still disagrees cannot turn
// into a loop.
export function OrganizationSync({ rendered }: { rendered: string | null }) {
  const router = useRouter();
  const pathname = usePathname();
  const organizations = useOrganizations();
  const active = useActiveOrganizationId();
  const remembered = organizationToRemember(pathname, organizations, active);
  const stale = staleShellOrganization(pathname, rendered, organizations);

  // Ahead of the refresh below, which is then asked for with it. On every
  // path, not only when the organization changes: another tab may have moved
  // the cookie since, and it follows whichever one was navigated in last.
  useEffect(() => {
    if (remembered === null) return;
    document.cookie = activeOrganizationCookie(remembered, window.location.protocol === "https:");
  }, [remembered, pathname]);

  useEffect(() => {
    if (stale !== null) router.refresh();
  }, [stale, router]);

  return null;
}
