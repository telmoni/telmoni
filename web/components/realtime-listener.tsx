"use client";

import { useEffect, useRef } from "react";
import { z } from "zod";
import { usePathname, useRouter } from "next/navigation";
import {
  useActiveOrganizationId,
  useIncomingInvites,
  useOrganizations,
  useProjects,
  useAddIncomingInvite,
  useRemoveIncomingInvite,
} from "@/lib/store";
import { organizationMatches, projectMatches } from "@/lib/slug";
import { RealtimeEventDataSchema } from "@/lib/events/types";

const RETRY_MS = 30_000;

const ROSTER_PAGES = /^\/[^/]+(?:\/[^/]+)?\/(members|audit-log)$/;

function matchesProject(
  pathname: string,
  projectId: string,
  organizationId?: string,
  projects: readonly { id: string; slug?: string | null }[] = [],
  organizations: readonly { organizationId: string; slug?: string | null }[] = [],
): boolean {
  if (pathname === `/${projectId}` || pathname.startsWith(`/${projectId}/`)) {
    return true;
  }
  const parts = pathname.split("/").filter(Boolean);
  if (parts.length >= 2) {
    const rawOrg = parts[0];
    const rawPrj = parts[1];
    const orgMatches =
      !organizationId ||
      rawOrg === organizationId ||
      organizations.some((o) => o.organizationId === organizationId && organizationMatches(o, rawOrg));
    const prjMatches =
      rawPrj === projectId ||
      projects.some((p) => p.id === projectId && projectMatches(p, rawPrj));
    if (orgMatches && prjMatches) return true;
  }
  if (parts.length >= 1) {
    const rawPrj = parts[0];
    if (projects.some((p) => p.id === projectId && projectMatches(p, rawPrj))) {
      return true;
    }
  }
  return false;
}

function payload<K extends keyof typeof RealtimeEventDataSchema>(
  e: MessageEvent,
  kind: K,
): z.infer<(typeof RealtimeEventDataSchema)[K]> | undefined {
  let body: unknown;
  try {
    body = JSON.parse(e.data);
  } catch {
    return undefined;
  }
  const parsed = RealtimeEventDataSchema[kind].safeParse(body);
  return parsed.success
    ? (parsed.data as z.infer<(typeof RealtimeEventDataSchema)[K]>)
    : undefined;
}

export function RealtimeListener() {
  const router = useRouter();
  const pathname = usePathname();
  const addInvite = useAddIncomingInvite();
  const removeInvite = useRemoveIncomingInvite();
  const memberships = useOrganizations();
  const incomingInvites = useIncomingInvites();
  const projects = useProjects();
  const activeOrganizationId = useActiveOrganizationId();
  const esRef = useRef<EventSource | null>(null);

  const pathnameRef = useRef(pathname);
  const invitesRef = useRef(incomingInvites);
  const activeOrgIdRef = useRef(activeOrganizationId);
  const projectsRef = useRef(projects);
  const membershipsRef = useRef(memberships);
  useEffect(() => {
    pathnameRef.current = pathname;
    invitesRef.current = incomingInvites;
    activeOrgIdRef.current = activeOrganizationId;
    projectsRef.current = projects;
    membershipsRef.current = memberships;
  });

  const membershipKey = memberships
    .map((m) => m.organizationId)
    .sort()
    .join("\n");

  useEffect(() => {
    let active = true;
    let everOpened = false;
    let refused = false;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;

    function settled(inviteId: string | undefined) {
      if (
        inviteId !== undefined &&
        invitesRef.current.some((invite) => invite.id === inviteId)
      ) {
        removeInvite(inviteId);
        router.refresh();
        return;
      }
      if (ROSTER_PAGES.test(pathnameRef.current)) router.refresh();
    }

    function connect() {
      if (!active) return;
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (esRef.current) {
        esRef.current.close();
      }

      const es = new EventSource("/api/events");
      esRef.current = es;

      es.addEventListener("open", () => {
        if (everOpened || refused) router.refresh();
        everOpened = true;
        refused = false;
      });

      es.addEventListener("invite:created", (e: MessageEvent) => {
        const invite = payload(e, "invite:created");
        if (!invite) return;
        addInvite(invite);
        router.refresh();
      });

      es.addEventListener("invite:revoked", (e: MessageEvent) => {
        settled(payload(e, "invite:revoked")?.inviteId);
      });

      es.addEventListener("invite:resolved", (e: MessageEvent) => {
        settled(payload(e, "invite:resolved")?.inviteId);
      });

      es.addEventListener("invite:sent", () => {
        if (ROSTER_PAGES.test(pathnameRef.current)) router.refresh();
      });

      // An offer made to you, withdrawn, declined, or an organization you are
      // in changing hands: every role on the page may have moved.
      es.addEventListener("ownership:changed", () => {
        router.refresh();
      });

      // You were removed from a project or organization: eject immediately
      // if currently viewing that project/organization, otherwise refresh.
      es.addEventListener("membership:removed", (e: MessageEvent) => {
        const data = payload(e, "membership:removed");
        if (!data) return;

        const isAccountPage = pathnameRef.current.startsWith("/account");
        const isCurrentProject = Boolean(
          data.projectId &&
            matchesProject(
              pathnameRef.current,
              data.projectId,
              data.organizationId,
              projectsRef.current,
              membershipsRef.current,
            ),
        );
        const isCurrentOrg = Boolean(
          !data.projectId &&
            (data.organizationId === activeOrgIdRef.current ||
              pathnameRef.current.startsWith(`/${data.organizationId}`) ||
              membershipsRef.current.some(
                (o) =>
                  o.organizationId === data.organizationId &&
                  (pathnameRef.current === `/${o.slug}` ||
                    pathnameRef.current.startsWith(`/${o.slug}/`)),
              )) &&
            !isAccountPage,
        );

        if (isCurrentProject || isCurrentOrg) {
          router.replace("/console");
        } else {
          router.refresh();
        }
      });

      es.addEventListener("close", () => {
        active = false;
        es.close();
        esRef.current = null;
      });

      es.onerror = () => {
        if (!active || es.readyState !== EventSource.CLOSED) return;
        refused = true;
        retryTimer = setTimeout(connect, RETRY_MS);
      };
    }

    connect();

    return () => {
      active = false;
      if (retryTimer) clearTimeout(retryTimer);
      if (esRef.current) {
        esRef.current.close();
        esRef.current = null;
      }
    };
  }, [router, addInvite, removeInvite, membershipKey]);

  return null;
}
