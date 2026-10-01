"use client";

import { useEffect, useRef } from "react";
import { z } from "zod";
import { usePathname, useRouter } from "next/navigation";
import {
  useIncomingInvites,
  useOrganizations,
  useAddIncomingInvite,
  useRemoveIncomingInvite,
} from "@/lib/store";
import { RealtimeEventDataSchema } from "@/lib/events/types";

const RETRY_MS = 30_000;

const ROSTER_PAGES = /^\/[^/]+\/(members|audit-log)$/;

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
  const esRef = useRef<EventSource | null>(null);

  const pathnameRef = useRef(pathname);
  const invitesRef = useRef(incomingInvites);
  useEffect(() => {
    pathnameRef.current = pathname;
    invitesRef.current = incomingInvites;
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
