"use client";

import { useEffect, useRef } from "react";
import { z } from "zod";
import { usePathname, useRouter } from "next/navigation";
import {
  useActiveOrganization,
  useActiveOrganizationId,
  useIncomingInvites,
  useOrganizations,
  useProjects,
  useAddIncomingInvite,
  useRemoveIncomingInvite,
} from "@/lib/store";
import { projectAt } from "@/lib/console-nav";
import { RealtimeEventDataSchema } from "@/lib/events/types";
import { projectPath } from "@/lib/slug";

const RETRY_MS = 30_000;

// A roster or an audit log: a project's, or the organization's under `~`.
const ROSTER_PAGES = /^\/[^/]+\/[^/]+\/(members|audit-log)$/;

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
  const activeOrganizationId = useActiveOrganizationId();
  // Events name a project by id and the path names it by slug: the id of the
  // one on screen, out of the listing of the organization the console stands in.
  const organization = useActiveOrganization()?.slug ?? null;
  const projects = useProjects();
  const projectId = projectAt(pathname, organization, projects)?.id ?? null;
  const esRef = useRef<EventSource | null>(null);

  const pathnameRef = useRef(pathname);
  const invitesRef = useRef(incomingInvites);
  const activeOrgIdRef = useRef(activeOrganizationId);
  const projectIdRef = useRef(projectId);
  useEffect(() => {
    pathnameRef.current = pathname;
    invitesRef.current = incomingInvites;
    activeOrgIdRef.current = activeOrganizationId;
    projectIdRef.current = projectId;
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
      es.addEventListener("ownership:changed", (e: MessageEvent) => {
        const data = payload(e, "ownership:changed");
        // The project on screen was handed to another organization, and this
        // path no longer names it: on to where it is now, by id, which the
        // console redirects to the slugs it goes by there.
        if (
          data?.projectId &&
          data.projectId === projectIdRef.current &&
          data.organizationId !== activeOrgIdRef.current
        ) {
          router.replace(projectPath(data.organizationId, data.projectId));
          return;
        }
        router.refresh();
      });

      // You were removed from a project or organization: eject immediately
      // if currently viewing that project/organization, otherwise refresh.
      es.addEventListener("membership:removed", (e: MessageEvent) => {
        const data = payload(e, "membership:removed");
        if (!data) return;

        const isAccountPage = pathnameRef.current.startsWith("/account");
        const isCurrentProject = Boolean(
          data.projectId && data.projectId === projectIdRef.current,
        );
        const isCurrentOrg = Boolean(
          !data.projectId &&
            data.organizationId === activeOrgIdRef.current &&
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

    const handleVisibilityOrOnline = () => {
      if (document.visibilityState === "visible" && active) {
        if (!esRef.current || esRef.current.readyState === EventSource.CLOSED) {
          connect();
        }
      }
    };

    document.addEventListener("visibilitychange", handleVisibilityOrOnline);
    window.addEventListener("online", handleVisibilityOrOnline);

    return () => {
      active = false;
      if (retryTimer) clearTimeout(retryTimer);
      document.removeEventListener("visibilitychange", handleVisibilityOrOnline);
      window.removeEventListener("online", handleVisibilityOrOnline);
      if (esRef.current) {
        esRef.current.close();
        esRef.current = null;
      }
    };
  }, [addInvite, removeInvite, router, membershipKey]);

  return null;
}
