"use client";

import { useRouter } from "next/navigation";
import { useEffect, useRef } from "react";
import { toast } from "sonner";

import { NOTIFICATIONS_HREF } from "@/lib/console-nav";
import { inviterText } from "@/lib/notifications";
import { useIncomingInvites } from "@/lib/store";

export function getInviteToastId(inviteId: string) {
  return `invite-${inviteId}`;
}

export function InviteToastNotifier() {
  const rawInvites = useIncomingInvites();
  const router = useRouter();

  const activeToastIdsRef = useRef<Set<string>>(new Set());
  const dismissedInviteIdsRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const invites = rawInvites ?? [];

    if (invites.length === 0) {
      for (const id of activeToastIdsRef.current) {
        toast.dismiss(getInviteToastId(id));
      }
      activeToastIdsRef.current.clear();
      return;
    }

    const currentInviteIds = new Set(invites.map((i) => i.id));

    for (const id of Array.from(activeToastIdsRef.current)) {
      if (!currentInviteIds.has(id)) {
        toast.dismiss(getInviteToastId(id));
        activeToastIdsRef.current.delete(id);
      }
    }

    for (const invite of invites) {
      if (dismissedInviteIdsRef.current.has(invite.id)) continue;
      if (activeToastIdsRef.current.has(invite.id)) continue;

      const title = `Invitation to join ${invite.targetName}`;
      const description = `${inviterText(invite)} invited you as ${invite.role}.`;
      const toastId = getInviteToastId(invite.id);

      toast(title, {
        id: toastId,
        description,
        duration: 15000,
        action: {
          label: "Review",
          // ⚠ **GOES TO THE PAGE, not the account menu.** This used to dispatch
          // ACCOUNT_MENU_EVENT, which opened the header dropdown — and the
          // dropdown's invitation rows are themselves links to this same href,
          // so "Review" cost a press and arrived nowhere. Accept and Decline
          // live only on `/account/notifications`; a control called Review that
          // does not reach them is a dead end dressed as an action.
          onClick: () => router.push(NOTIFICATIONS_HREF),
        },
        onDismiss: () => {
          dismissedInviteIdsRef.current.add(invite.id);
          activeToastIdsRef.current.delete(invite.id);
        },
        onAutoClose: () => {
          activeToastIdsRef.current.delete(invite.id);
        },
      });

      activeToastIdsRef.current.add(invite.id);
    }
    // `router` is stable across renders in the App Router, so naming it here
    // satisfies the lint rule without re-running this effect and re-toasting.
  }, [rawInvites, router]);

  return null;
}

