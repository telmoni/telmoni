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
import { movedPath, projectAt } from "@/lib/console-nav";
import { RealtimeEventDataSchema } from "@/lib/events/types";
import { LIVE_HOLD_MS, liveRefresher } from "@/lib/live-refresh";
import { projectPath } from "@/lib/slug";
import { PEEK_PARAM, PEEKED_RUN_LANDED } from "@/lib/traces";
import { listIsEmpty, tracesArrived } from "@/lib/use-new-traces";

const RETRY_MS = 30_000;

// A roster or an audit log: a project's, or the organization's own.
const ROSTER_PAGES = /^\/[^/]+(?:\/[^/]+)?\/(members|audit-log)$/;

// A page reading a project's spans: its traces, sessions or users, the list
// or one of them. A trace's page names its run, which a nudge is compared
// with; `traces/export` is the download's route, not a page.
const SPAN_PAGES = /^\/[^/]+\/[^/]+\/(traces|sessions|users)(?:\/(?!export$)([^/]+))?$/;

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

/// `liveHoldMs` is how long a page reading spans holds between the redraws
/// ingest's nudges ask for: the layout passes the deployment's
/// (`LIVE_REFRESH_HOLD_SECS`), since this runs in the browser and reads no
/// environment of its own.
export function RealtimeListener({ liveHoldMs = LIVE_HOLD_MS }: { liveHoldMs?: number }) {
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
    // A page reading spans is drawn only while it is in view, whether a nudge
    // asks now or a hold's end does, as the Traces timer leaves a hidden tab
    // alone; what landed meanwhile is drawn once when the tab returns.
    let dirty = false;
    const live = liveRefresher(() => {
      if (document.visibilityState === "visible") router.refresh();
      else dirty = true;
    }, liveHoldMs);
    // The trace the list's peek holds open is read again when its own run
    // lands, as a trace's page is drawn for its own run: held as a page is,
    // and only while in view.
    let peekDirty = false;
    const peekLive = liveRefresher(() => {
      if (document.visibilityState === "visible") window.dispatchEvent(new Event(PEEKED_RUN_LANDED));
      else peekDirty = true;
    }, liveHoldMs);

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

      // An offer made to you, withdrawn, declined, an organization you are in
      // changing hands, or one you founded in another tab: every role on the
      // page may have moved, and the switcher's list with them.
      es.addEventListener("ownership:changed", (e: MessageEvent) => {
        const data = payload(e, "ownership:changed");
        // The project on screen was handed to another organization, and this
        // path no longer names it: on to where it is now, by the slugs the
        // event carries. Without them there is nowhere to spell, and the
        // refresh answers "not found" here, which is the truth of this path.
        if (
          data?.projectId &&
          data.projectId === projectIdRef.current &&
          data.organizationId !== activeOrgIdRef.current &&
          data.organizationSlug &&
          data.projectSlug
        ) {
          router.replace(projectPath(data.organizationSlug, data.projectSlug));
          return;
        }
        router.refresh();
      });

      // A slug this path may be spelled with moved — a project renamed, an
      // organization's URL changed: on to where the page is now. Everything
      // else on screen still links by the old slug, so it is asked for again
      // either way — after the move, never before:
      // the old path names nothing any more.
      es.addEventListener("slug:moved", (e: MessageEvent) => {
        const moved = payload(e, "slug:moved");
        if (!moved) return;
        const path = movedPath(pathnameRef.current, moved);
        if (path) router.replace(path + window.location.search);
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

      // Your role in a project or organization changed, by somebody else or
      // in another tab: the page's controls follow the role the server now
      // answers, so it is asked for again.
      es.addEventListener("membership:changed", () => {
        router.refresh();
      });

      // A project's spans arrived: a page reading them is drawn again, the
      // nudges coalesced (`liveRefresher`) — but the Traces list, whose rows
      // would move under the reader's hand, counts them in a bar above itself
      // instead (`NewTraces`), unless it is empty and has nothing to move;
      // and a trace's page is drawn for its own run alone.
      es.addEventListener("spans:ingested", (e: MessageEvent) => {
        const data = payload(e, "spans:ingested");
        if (!data || data.projectId !== projectIdRef.current) return;
        const page = SPAN_PAGES.exec(pathnameRef.current);
        if (!page) return;
        const [, section, one] = page;
        if (section === "traces" && !one) {
          const peeked = new URLSearchParams(window.location.search).get(PEEK_PARAM);
          if (peeked && (data.more || data.runs.includes(peeked))) peekLive.nudge();
        }
        if (section === "traces" && !one && !listIsEmpty(data.projectId)) {
          tracesArrived(data.projectId, data.runs, data.more);
          return;
        }
        if (section === "traces" && one && !data.more && !data.runs.includes(one)) return;
        live.nudge();
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
        if (dirty) {
          dirty = false;
          live.flush();
        }
        if (peekDirty) {
          peekDirty = false;
          peekLive.flush();
        }
      }
    };

    document.addEventListener("visibilitychange", handleVisibilityOrOnline);
    window.addEventListener("online", handleVisibilityOrOnline);

    return () => {
      active = false;
      live.dispose();
      peekLive.dispose();
      if (retryTimer) clearTimeout(retryTimer);
      document.removeEventListener("visibilitychange", handleVisibilityOrOnline);
      window.removeEventListener("online", handleVisibilityOrOnline);
      if (esRef.current) {
        esRef.current.close();
        esRef.current = null;
      }
    };
  }, [addInvite, removeInvite, router, membershipKey, liveHoldMs]);

  return null;
}
