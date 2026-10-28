"use client";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { EnumSelect } from "@/components/enum-select";
import { Button } from "@/components/ui/button";
import type { ActionResult } from "@/lib/action-result";
import { personDetail, personName } from "@/lib/identity";
import { roleLabel } from "@/lib/role-label";
import { type GivenRole, owns } from "@/lib/roles";
import { Role } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

// A roster's rows, the organization's and a project's alike: a seat with its
// role and the offer of the whole to an admin, and an invitation. Each control
// calls the action its page bound to the roster it rendered, which acts on
// that one or on nothing.

/** Which roster a row is of. */
export type RosterKind = "organization" | "project";

/** A seat as either roster lists it. */
export type Seat = {
  id: string;
  member_id: string;
  email: string;
  display_name?: string | null;
  role: Role;
  created_at: string;
  /** When the owner's live offer of the whole to this admin lapses. */
  offer_expires_at?: string | null;
};

/** An invitation as either roster lists it. */
export type Invitation = { id: string; email: string; role: Role; expires_at: string };

/** What the roster's actions do to a seat, bound to the roster by its page. */
export type SeatActions = {
  setRole: (memberId: string, role: GivenRole) => Promise<ActionResult>;
  remove: (memberId: string) => Promise<ActionResult>;
  offer: (memberId: string) => Promise<ActionResult>;
  withdraw: () => Promise<ActionResult>;
};

type Words = {
  owner: string;
  offered: string;
  transfer: string;
  hand: (name: string) => string;
  handed: string;
  withdraw: (email: string) => string;
  withdrawTitle: string;
  withdrawn: string;
  remove: (name: string) => string;
  removed: string;
};

/** What a row says of the whole it is a seat on. */
const WORDS = {
  organization: {
    owner: "Organization Owner",
    offered: "Ownership offered",
    transfer: "Transfer ownership",
    hand: (name) => `Hand this organization to ${name}?`,
    handed:
      "They become the owner when they accept: deleting the organization, handing it on, and deleting its projects or handing them to other organizations become theirs alone. You stay on as an admin of the organization and of every project, managing members, keys and connectors as before. The offer lapses in 7 days if they do nothing.",
    withdraw: (email) => `Withdraw the ownership offer to ${email}`,
    withdrawTitle: "Withdraw the ownership offer?",
    withdrawn: "The organization stays yours, and they can no longer accept it.",
    remove: (name) => `Remove ${name} from organization?`,
    removed:
      "They will immediately lose access to all projects and resources in this organization.",
  },
  project: {
    owner: "Project Owner",
    offered: "Project offered",
    transfer: "Transfer project",
    hand: (name) => `Hand this project to ${name}?`,
    handed:
      "They become its owner when they accept: it moves into an organization they own, with its members and API keys. Its Slack and webhook connectors stay with your organization and are disconnected. You stay on as an admin of the project, and join their organization as a member. The offer lapses in 7 days if they do nothing.",
    withdraw: (email) => `Withdraw the project offer to ${email}`,
    withdrawTitle: "Withdraw the offer of this project?",
    withdrawn: "The project stays yours, and they can no longer accept it.",
    remove: (name) => `Remove ${name}?`,
    removed: "They lose access to this project immediately.",
  },
} satisfies Record<RosterKind, Words>;

export function MemberRow({
  of,
  member,
  act,
  canManage = true,
  isOwnerCaller = false,
  self,
}: {
  of: RosterKind;
  member: Seat;
  act: SeatActions;
  /** Whether the reader manages the roster: a project's member only reads it. */
  canManage?: boolean;
  isOwnerCaller?: boolean;
  /** The caller's own row, on a project: its action is Leave, never Remove. */
  self?: { name: string; leave: () => Promise<ActionResult> };
}) {
  const { pending, error, run } = useAction();
  const words = WORDS[of];
  const person = { displayName: member.display_name, email: member.email };
  const isOwner = owns(member.role);
  // The seat's role as one that is given: an owner's is nobody's to set.
  const given = member.role === Role.Owner ? null : member.role;
  // Only an admin can be handed the whole, and only the owner hands it.
  const canOffer = canManage && isOwnerCaller && member.role === Role.Admin;
  const offered = Boolean(member.offer_expires_at);
  const control = (label: string, text: string) => (
    <Button variant="outline" size="sm" className="h-9 text-xs" disabled={pending} aria-label={label}>
      {text}
    </Button>
  );

  return (
    <tr>
      <Td className="font-medium">
        <div className="flex items-center gap-2">
          <span>{personName(person)}</span>
          {isOwner && (
            <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
              {words.owner}
            </Badge>
          )}
          {offered && member.offer_expires_at && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              {words.offered} · lapses <LocalTime iso={member.offer_expires_at} mode="date" />
            </Badge>
          )}
        </div>
        {personDetail(person) && (
          <span className="block text-xs font-normal text-muted-foreground wrap-anywhere">
            {personDetail(person)}
          </span>
        )}
      </Td>
      <Td>
        {given === null ? (
          <span className="text-xs font-medium text-muted-foreground">Owner</span>
        ) : canManage ? (
          <EnumSelect
            value={given}
            options={[Role.Admin, Role.Member]}
            label={roleLabel}
            onChange={(role) => run(() => act.setRole(member.member_id, role))}
            disabled={pending}
            className="h-9 w-32 text-xs"
            ariaLabel={`Role for ${member.email}`}
          />
        ) : (
          <span className="text-xs text-muted-foreground">{roleLabel(given)}</span>
        )}
        {error && <span className="ml-2 text-xs text-destructive">{error}</span>}
      </Td>
      <Td className="text-xs text-muted-foreground whitespace-nowrap">
        <LocalTime iso={member.created_at} mode="date" />
      </Td>
      <Td className="text-right whitespace-nowrap">
        <div className="flex items-center justify-end gap-3">
          {canOffer && !offered && (
            <ConfirmDialog
              trigger={control(`${words.transfer} to ${member.email}`, words.transfer)}
              title={words.hand(personName(person))}
              description={words.handed}
              confirmLabel="Send offer"
              onConfirm={() => run(() => act.offer(member.member_id))}
            />
          )}
          {canOffer && offered && (
            <ConfirmDialog
              trigger={control(words.withdraw(member.email), "Withdraw offer")}
              title={words.withdrawTitle}
              description={words.withdrawn}
              confirmLabel="Withdraw"
              onConfirm={() => run(() => act.withdraw())}
            />
          )}
          {!isOwner && self && (
            <ConfirmDialog
              trigger={control(`Leave ${self.name}`, "Leave")}
              title={`Leave ${self.name}?`}
              description="You will lose access to this project immediately. An owner or admin can add you back."
              confirmLabel="Leave"
              onConfirm={() => run(() => self.leave())}
            />
          )}
          {!isOwner && !self && canManage && (
            <ConfirmDialog
              trigger={control(`Remove ${member.email}`, "Remove")}
              title={words.remove(personName(person))}
              description={words.removed}
              confirmLabel="Remove"
              onConfirm={() => run(() => act.remove(member.member_id))}
            />
          )}
        </div>
      </Td>
    </tr>
  );
}

export function InviteRow({
  invite,
  canManage = true,
  revoke,
}: {
  invite: Invitation;
  canManage?: boolean;
  /** The invitation withdrawn, bound to the roster by its page. */
  revoke: (inviteId: string) => Promise<ActionResult>;
}) {
  const { pending, error, run } = useAction();

  return (
    <tr>
      <Td className="font-medium">{invite.email}</Td>
      <Td className="text-xs text-muted-foreground">{roleLabel(invite.role)}</Td>
      <Td className="text-xs text-muted-foreground whitespace-nowrap">
        Expires <LocalTime iso={invite.expires_at} mode="date" />
      </Td>
      <Td className="text-right whitespace-nowrap">
        {error && <span className="mr-2 text-xs text-destructive">{error}</span>}
        {canManage && (
          <ConfirmDialog
            trigger={
              <Button
                variant="outline"
                size="sm"
                className="h-9 text-xs"
                disabled={pending}
                aria-label={`Withdraw the invitation to ${invite.email}`}
              >
                Withdraw
              </Button>
            }
            title={`Withdraw the invitation to ${invite.email}?`}
            description="Their link stops working straight away. You can invite them again later."
            confirmLabel="Withdraw"
            onConfirm={() => run(() => revoke(invite.id))}
          />
        )}
      </Td>
    </tr>
  );
}
