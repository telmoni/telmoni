import type { ReactNode } from "react";

import { DataTable, THead, Th } from "@/components/data-table";
import { InviteRow, type Invitation, type RosterKind } from "@/components/members/rows";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import type { ActionResult } from "@/lib/action-result";
import { roleLabel } from "@/lib/role-label";
import { Role } from "@/lib/types/enums";

// What the two Members pages lay out alike: the three roles explained, the
// seats in a table and the outstanding invitations under it.

/** What a role lets its holder do here: the badge beside its name, and a line. */
export type RoleCard = { badge: string; body: ReactNode };

export function RoleCards({ cards }: { cards: Record<Role, RoleCard> }) {
  return (
    <section className="grid gap-3 lg:grid-cols-3">
      {[Role.Owner, Role.Admin, Role.Member].map((role) => (
        <Card key={role} className="gap-1.5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-medium text-sm">{roleLabel(role)}</span>
            <Badge variant="secondary" className="text-xs shrink-0">
              {cards[role].badge}
            </Badge>
          </div>
          <p className="text-xs text-muted-foreground">{cards[role].body}</p>
        </Card>
      ))}
    </section>
  );
}

/** The seats, in fixed columns: an admin's row grows a Transfer button, and
 *  with the widths left to the content, changing a role slid the Role and
 *  Added columns under the cursor. The last column fits the widest pair of
 *  buttons a row can carry. */
export function RosterTable({ children }: { children: ReactNode }) {
  return (
    <DataTable className="table-fixed min-w-3xl">
      <THead>
        <Th>Member</Th>
        <Th className="w-40">Role</Th>
        <Th className="w-32">Added</Th>
        <Th className="w-72" />
      </THead>
      <tbody>{children}</tbody>
    </DataTable>
  );
}

/** The outstanding invitations, while there are any. */
export function Invited({
  of,
  invites,
  canManage = true,
  revoke,
}: {
  of: RosterKind;
  invites: readonly Invitation[];
  canManage?: boolean;
  revoke: (inviteId: string) => Promise<ActionResult>;
}) {
  if (invites.length === 0) return null;
  return (
    <section className="grid gap-3">
      <h2 className="text-sm font-medium">Invited</h2>
      <DataTable>
        <THead>
          <Th>Email address</Th>
          <Th>Role</Th>
          <Th>Invitation</Th>
          <Th />
        </THead>
        <tbody>
          {invites.map((i) => (
            <InviteRow key={i.id} invite={i} canManage={canManage} revoke={revoke} />
          ))}
        </tbody>
      </DataTable>
      <p className="text-xs text-muted-foreground">
        An invitation is an offer, not access — nobody on this list has access to your {of}.
        Each link works once, only for the address it was sent to, and expires on its own.
      </p>
    </section>
  );
}
