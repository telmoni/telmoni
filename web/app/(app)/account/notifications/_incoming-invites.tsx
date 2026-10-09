"use client";

import { useRouter } from "next/navigation";
import { Check, X } from "lucide-react";
import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { IncomingInvite } from "@/lib/types/incoming-invite";
import { inviterText } from "@/lib/notifications";
import { useRemoveIncomingInvite } from "@/lib/store";
import { roleLabel } from "@/lib/role-label";

import {
  acceptIncomingInviteAction,
  declineIncomingInviteAction,
} from "./invite-actions";


export function IncomingInvitesSection({
  invites,
}: {
  invites: readonly IncomingInvite[];
}) {
  if (invites.length === 0) return null;

  return (
    <section className="grid gap-3" id="pending-invitations" data-testid="pending-invitations">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-medium">Pending invitations</h2>
        <Badge variant="secondary" className="text-xs">
          {invites.length}
        </Badge>
      </div>
      <DataTable>
        <THead>
          <Th>Invited to</Th>
          <Th>Invited by</Th>
          <Th>Role</Th>
          <Th>Expires</Th>
          <Th className="text-right" />
        </THead>
        <tbody>
          {invites.map((invite) => (
            <IncomingInviteRow key={invite.id} invite={invite} />
          ))}
        </tbody>
      </DataTable>
      <p className="text-xs text-muted-foreground">
        Accepting an invitation grants you access to that organization or project. Declining
        withdraws the offer.
      </p>
    </section>
  );
}

function IncomingInviteRow({ invite }: { invite: IncomingInvite }) {
  const router = useRouter();
  const removeIncomingInvite = useRemoveIncomingInvite();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  function settle(run: () => Promise<{ error: string | null }>, failure: string) {
    setError(null);
    start(async () => {
      try {
        const res = await run();
        if (res.error) {
          setError(res.error);
          return;
        }
        removeIncomingInvite(invite.id);
        router.refresh();
      } catch {
        setError(failure);
      }
    });
  }

  const inviter = inviterText(invite);

  return (
    <tr>
      <Td className="font-medium">
        <div className="flex items-center gap-2">
          <span>{invite.targetName}</span>
          <Badge variant="secondary" className="text-[10px] font-normal uppercase tracking-wider">
            {invite.scope}
          </Badge>
        </div>
      </Td>
      <Td className="text-muted-foreground">
        <span>{inviter}</span>
        {invite.inviterEmail && inviter !== invite.inviterEmail && (
          <span className="block text-xs text-muted-foreground/70">
            {invite.inviterEmail}
          </span>
        )}
      </Td>
      <Td className="text-xs text-muted-foreground">
        {roleLabel(invite.role)}
      </Td>
      <Td className="text-xs text-muted-foreground whitespace-nowrap">
        <LocalTime iso={invite.expiresAt} mode="date" />
      </Td>
      <Td className="text-right whitespace-nowrap">
        <div className="flex items-center justify-end gap-3">
          {error && <span className="text-xs text-destructive mr-2">{error}</span>}
          <Button
            size="sm"
            className="h-9 text-xs gap-1"
            disabled={pending}
            onClick={() =>
              settle(
                () => acceptIncomingInviteAction(invite.id),
                "Failed to accept invitation. Try again.",
              )
            }
          >
            <Check className="size-3.5" />
            Accept
          </Button>
          <ConfirmDialog
            trigger={
              <Button
                variant="outline"
                size="sm"
                className="h-9 text-xs gap-1 text-muted-foreground hover:text-foreground"
                disabled={pending}
              >
                <X className="size-3.5" />
                Decline
              </Button>
            }
            title={`Decline invitation to join ${invite.targetName}?`}
            description="You will not receive access to it. You can be invited again later if needed."
            confirmLabel="Decline"
            onConfirm={() =>
              settle(
                () => declineIncomingInviteAction(invite.id),
                "Failed to decline invitation. Try again.",
              )
            }
          />
        </div>
      </Td>
    </tr>
  );
}
