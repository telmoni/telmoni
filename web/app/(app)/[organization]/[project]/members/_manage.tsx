"use client";

import { InviteSentDialog } from "@/components/members/invite-sent-dialog";

import { UserPlus } from "lucide-react";
import { startTransition, useState } from "react";
import { toast } from "sonner";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { PageAction } from "@/components/page-action";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { personDetail, personName } from "@/lib/identity";
import type { Invite, Member } from "@/lib/server/data";
import { roleLabel } from "@/lib/role-label";
import { type GivenRole, owns } from "@/lib/roles";
import { Role } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import {
  addMemberAction,
  cancelProjectOfferAction,
  inviteMemberAction,
  leaveProjectAction,
  offerProjectAction,
  removeMemberAction,
  revokeInviteAction,
  updateMemberRoleAction,
} from "./actions";


/// A member of the organization holding no seat on the project yet: the
/// owner and admins may seat them here with nothing to accept.
export type Candidate = { memberId: string; email: string; displayName: string | null };

type Way = "roster" | "email";
const WAYS: readonly Choice<Way>[] = [
  { value: "roster", label: "From the organization" },
  { value: "email", label: "By email" },
];

// Each of these takes the project its page rendered, by id: the actions act on
// that one or on nothing.
export function AddMember({
  projectId,
  candidates,
  rosterUnread = false,
  inviteClosed,
}: {
  projectId: string;
  candidates: readonly Candidate[];
  /** The organization's roster was this person's to read and could not be. */
  rosterUnread?: boolean;
  /** Why an invitation would be refused here — the organization's ceiling —
   *  when it would: the roster's seats stay open. */
  inviteClosed?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <PageAction primary="Add member" onClick={() => setOpen(true)}>
        <UserPlus className="size-4" />
        Add member
      </PageAction>
      <AddDialog
        key={`add-${open}`}
        projectId={projectId}
        candidates={candidates}
        rosterUnread={rosterUnread}
        inviteClosed={inviteClosed}
        open={open}
        onOpenChange={setOpen}
      />
    </>
  );
}

// Two ways in. A member of the organization is seated at once, with nothing
// to accept — offered while there is one left to seat, to the organization's
// owner and admins, who alone are handed its roster. Anybody else is invited
// by email, as before. One role serves both.
function AddDialog({
  projectId,
  candidates,
  rosterUnread,
  inviteClosed,
  open,
  onOpenChange,
}: {
  projectId: string;
  candidates: readonly Candidate[];
  rosterUnread: boolean;
  inviteClosed: string | undefined;
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const [way, setWay] = useState<Way>(candidates.length > 0 ? "roster" : "email");
  // Nobody chosen until somebody is: a seat given mails its person.
  const [memberId, setMemberId] = useState("");
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<GivenRole>(Role.Member);
  const [link, setLink] = useState<string | null>(null);
  const { pending, error, setError, run } = useAction();
  const chosen = candidates.find((c) => c.memberId === memberId);
  const fromRoster = way === "roster" && candidates.length > 0;

  function submit() {
    if (fromRoster) {
      if (!chosen) return;
      run(
        () => addMemberAction(projectId, chosen.memberId, role),
        () => {
          toast.success(
            `Added ${personName(chosen)} as ${role === Role.Admin ? "an admin" : "a member"}.`,
          );
          startTransition(() => onOpenChange(false));
        },
      );
      return;
    }
    run(
      () => inviteMemberAction(projectId, email, role),
      (r) => {
        // In the transition, with the invitations the action drew again.
        startTransition(() => setLink(r.link ?? null));
        toast.success(`Invitation sent to ${email}`);
      },
    );
  }

  if (link) {
    return (
      <InviteSentDialog
        open={open}
        onOpenChange={onOpenChange}
        email={email}
        link={link}
        scope="project"
      />
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a member</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {candidates.length > 0 && (
            <div className="grid gap-1.5">
              <span id="member-way" className="text-sm leading-none font-medium">
                Who
              </span>
              <div className="flex">
                <ChoiceGroup
                  labelledBy="member-way"
                  describedBy={inviteClosed ? "member-way-closed" : undefined}
                  options={
                    inviteClosed
                      ? WAYS.map((w) => (w.value === "email" ? { ...w, disabled: true } : w))
                      : WAYS
                  }
                  value={way}
                  onChange={(next) => {
                    // Held while an add is under way: its answer is that way's.
                    if (pending) return;
                    setWay(next);
                    setError(null);
                  }}
                />
              </div>
              {inviteClosed && (
                <p id="member-way-closed" className="text-sm text-muted-foreground" role="note">
                  {inviteClosed}
                </p>
              )}
            </div>
          )}
          {fromRoster ? (
            <>
              <DialogDescription>
                Somebody already in the organization is added at once, with
                nothing to accept, and told by email and in their notifications.
              </DialogDescription>
              <div className="grid gap-1.5">
                <Label htmlFor="member-person">Person</Label>
                <Select value={memberId} onValueChange={setMemberId}>
                  <SelectTrigger id="member-person" className="w-full">
                    <SelectValue placeholder="Choose somebody" />
                  </SelectTrigger>
                  <SelectContent>
                    {candidates.map((c) => (
                      <SelectItem key={c.memberId} value={c.memberId}>
                        {personName(c)}
                        {personDetail(c) ? ` — ${personDetail(c)}` : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </>
          ) : (
            <>
              <DialogDescription>
                By email address. They get a link, and nothing changes here until
                they accept it — whether or not they already have an organization.
              </DialogDescription>
              {rosterUnread && (
                <p className="text-sm text-muted-foreground" role="note">
                  The organization&rsquo;s members can&rsquo;t be read right now, so
                  adding one of them waits; reload in a moment.
                </p>
              )}
              <div className="grid gap-1.5">
                <Label htmlFor="member-email">Email address</Label>
                <Input
                  id="member-email"
                  type="email"
                  autoComplete="off"
                  value={email}
                  placeholder="dana@example.com"
                  onChange={(e) => setEmail(e.target.value)}
                />
              </div>
            </>
          )}
          <div className="grid gap-1.5">
            <Label htmlFor="member-role">Role</Label>
            <Select value={role} onValueChange={(v) => setRole(v as GivenRole)}>
              <SelectTrigger id="member-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={Role.Member}>
                  Member — read-only access
                </SelectItem>
                <SelectItem value={Role.Admin}>
                  Admin — read access and the audit log
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          {error && (
            <p className="text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </DialogBody>
        <DialogFooter>
          <Button
            onClick={submit}
            disabled={pending || (fromRoster ? !chosen : email.trim().length === 0)}
          >
            {fromRoster
              ? pending
                ? "Adding…"
                : "Add"
              : pending
                ? "Sending…"
                : "Send invitation"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function InviteRow({
  projectId,
  invite,
  canManage = true,
}: {
  projectId: string;
  invite: Invite;
  canManage?: boolean;
}) {
  const { pending, error, run } = useAction();

  return (
    <tr>
      <Td className="font-medium">{invite.email}</Td>
      <Td className="text-xs text-muted-foreground">
        {roleLabel(invite.role)}
      </Td>
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
            onConfirm={() => run(() => revokeInviteAction(projectId, invite.id))}
          />
        )}
      </Td>
    </tr>
  );
}


export type MemberRowProps = {
  projectId: string;
  projectName: string;
  member: Member;
  canManage?: boolean;
  isOwnerCaller?: boolean;
  /// The caller's own row: its action is Leave, never Remove.
  isSelf?: boolean;
};

export function MemberRow({
  projectId,
  projectName,
  member,
  canManage = true,
  isOwnerCaller = false,
  isSelf = false,
}: MemberRowProps) {
  const { pending, error, run: act } = useAction();

  const isOwner = owns(member.role);
  // Only an admin can be handed the project, and only the owner hands it.
  const canOffer = canManage && isOwnerCaller && !isOwner && member.role === Role.Admin;
  const offered = Boolean(member.offer_expires_at);

  return (
    <tr>
      <Td className="font-medium">
        <div className="flex items-center gap-2">
          <span>{personName(who(member))}</span>
          {isOwner && (
            <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
              Project Owner
            </Badge>
          )}
          {offered && member.offer_expires_at && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              Project offered · lapses{" "}
              <LocalTime iso={member.offer_expires_at} mode="date" />
            </Badge>
          )}
        </div>
        {personDetail(who(member)) && (
          <span className="block text-xs font-normal text-muted-foreground wrap-anywhere">
            {personDetail(who(member))}
          </span>
        )}
      </Td>
      <Td>
        {isOwner ? (
          <span className="text-xs font-medium text-muted-foreground">Owner</span>
        ) : canManage ? (
          <Select
            value={member.role}
            disabled={pending}
            onValueChange={(v) =>
              act(() => updateMemberRoleAction(projectId, member.member_id, v as GivenRole))
            }
          >
            <SelectTrigger
              className="h-9 w-32 text-xs"
              aria-label={`Role for ${member.email}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={Role.Member}>Member</SelectItem>
              <SelectItem value={Role.Admin}>Admin</SelectItem>
            </SelectContent>
          </Select>
        ) : (
          <span className="text-xs text-muted-foreground">{roleLabel(member.role)}</span>
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
              trigger={
                <Button
                  variant="outline"
                  size="sm"
                  className="h-9 text-xs"
                  disabled={pending}
                  aria-label={`Transfer project to ${member.email}`}
                >
                  Transfer project
                </Button>
              }
              title={`Hand this project to ${personName(who(member))}?`}
              description="They become its owner when they accept: it moves into an organization they own, with its members and API keys. Its Slack and webhook connectors stay with your organization and are disconnected. You stay on as an admin of the project, and join their organization as a member. The offer lapses in 7 days if they do nothing."
              confirmLabel="Send offer"
              onConfirm={() => act(() => offerProjectAction(projectId, member.member_id))}
            />
          )}
          {canOffer && offered && (
            <ConfirmDialog
              trigger={
                <Button
                  variant="outline"
                  size="sm"
                  className="h-9 text-xs"
                  disabled={pending}
                  aria-label={`Withdraw the project offer to ${member.email}`}
                >
                  Withdraw offer
                </Button>
              }
              title="Withdraw the offer of this project?"
              description="The project stays yours, and they can no longer accept it."
              confirmLabel="Withdraw"
              onConfirm={() => act(() => cancelProjectOfferAction(projectId))}
            />
          )}
          {!isOwner && isSelf && (
            <ConfirmDialog
              trigger={
                <Button
                  variant="outline"
                  size="sm"
                  className="h-9 text-xs"
                  disabled={pending}
                  aria-label={`Leave ${projectName}`}
                >
                  Leave
                </Button>
              }
              title={`Leave ${projectName}?`}
              description="You will lose access to this project immediately. An owner or admin can add you back."
              confirmLabel="Leave"
              onConfirm={() => act(() => leaveProjectAction(projectId, "project"))}
            />
          )}
          {!isOwner && !isSelf && canManage && (
            <ConfirmDialog
              trigger={
                <Button
                  variant="outline"
                  size="sm"
                  className="h-9 text-xs"
                  disabled={pending}
                  aria-label={`Remove ${member.email}`}
                >
                  Remove
                </Button>
              }
              title={`Remove ${personName(who(member))}?`}
              description="They lose access to this project immediately."
              confirmLabel="Remove"
              onConfirm={() => act(() => removeMemberAction(projectId, member.member_id))}
            />
          )}
        </div>
      </Td>
    </tr>
  );
}

function who(member: Member): { displayName?: string | null; email: string } {
  return { displayName: member.display_name, email: member.email };
}
