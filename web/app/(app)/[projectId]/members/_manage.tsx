"use client";

import { InviteSentDialog } from "@/components/members/invite-sent-dialog";

import { useParams, useRouter } from "next/navigation";
import { UserPlus } from "lucide-react";
import { useState, useTransition } from "react";
import { toast } from "sonner";

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

import {
  cancelProjectOfferAction,
  inviteMemberAction,
  offerProjectAction,
  removeMemberAction,
  revokeInviteAction,
  updateMemberRoleAction,
} from "./actions";

type Grantable = "admin" | "member";

export function AddMember() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <PageAction primary="Invite member" onClick={() => setOpen(true)}>
        <UserPlus className="size-4" />
        Invite member
      </PageAction>
      <AddDialog key={`add-${open}`} open={open} onOpenChange={setOpen} />
    </>
  );
}

function AddDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<Grantable>("member");
  const [error, setError] = useState<string | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const { projectId } = useParams<{ projectId: string }>();
  const [pending, start] = useTransition();

  function submit() {
    setError(null);
    start(async () => {
      try {
        const r = await inviteMemberAction(projectId, email, role);
        if (r.error) setError(r.error);
        else {
          setLink(r.link ?? null);
          toast.success(`Invitation sent to ${email}`);
          router.refresh();
        }
      } catch {
        setError("Network error. Try again.");
      }
    });
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
          <DialogTitle>Invite a member</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription>
            By email address. They get a link, and nothing changes here until
            they accept it — whether or not they already have an organization.
          </DialogDescription>
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
          <div className="grid gap-1.5">
            <Label htmlFor="member-role">Role</Label>
            <Select value={role} onValueChange={(v) => setRole(v as Grantable)}>
              <SelectTrigger id="member-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="member">
                  Member — read-only access
                </SelectItem>
                <SelectItem value="admin">
                  Admin — read access and the audit log
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          {error && <p className="text-sm text-destructive">{error}</p>}
        </DialogBody>
        <DialogFooter>
          <Button
            onClick={submit}
            disabled={pending || email.trim().length === 0}
          >
            {pending ? "Sending…" : "Send invitation"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function InviteRow({
  invite,
  canManage = true,
}: {
  invite: Invite;
  canManage?: boolean;
}) {
  const { projectId } = useParams<{ projectId: string }>();

  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  return (
    <tr>
      <Td className="font-medium">{invite.email}</Td>
      <Td className="text-xs text-muted-foreground">
        {invite.role ? roleLabel(invite.role) : "—"}
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
                className="h-8 text-xs"
                disabled={pending}
                aria-label={`Withdraw the invitation to ${invite.email}`}
              >
                Withdraw
              </Button>
            }
            title={`Withdraw the invitation to ${invite.email}?`}
            description="Their link stops working straight away. You can invite them again later."
            confirmLabel="Withdraw"
            onConfirm={() =>
              start(async () => {
                setError(null);
                try {
                  const r = await revokeInviteAction(projectId, invite.id);
                  if (r.error) setError(r.error);
                  else router.refresh();
                } catch {
                  setError("Network error. Try again.");
                }
              })
            }
          />
        )}
      </Td>
    </tr>
  );
}


export type MemberRowProps = {
  member: Member;
  canManage?: boolean;
  isOwnerCaller?: boolean;
};

export function MemberRow({
  member,
  canManage = true,
  isOwnerCaller = false,
}: MemberRowProps) {
  const { projectId } = useParams<{ projectId: string }>();

  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  function act(work: () => Promise<{ error: string | null }>) {
    start(async () => {
      setError(null);
      try {
        const r = await work();
        if (r.error) setError(r.error);
        else router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  const isOwner = member.is_owner || member.role === "owner";
  // Only an admin can be handed the project, and only the owner hands it.
  const canOffer = canManage && isOwnerCaller && !isOwner && member.role === "admin";
  const offered = Boolean(member.transfer_offer_expires_at);

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
          {offered && member.transfer_offer_expires_at && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              Project offered · lapses{" "}
              <LocalTime iso={member.transfer_offer_expires_at} mode="date" />
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
            value={member.role ?? "member"}
            disabled={pending}
            onValueChange={(v) =>
              act(() => updateMemberRoleAction(projectId, member.member_id, v as Grantable))
            }
          >
            <SelectTrigger
              className="h-8 w-32 text-xs"
              aria-label={`Role for ${member.email}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="member">Member</SelectItem>
              <SelectItem value="admin">Admin</SelectItem>
            </SelectContent>
          </Select>
        ) : (
          <span className="text-xs text-muted-foreground">{roleLabel(member.role ?? "member")}</span>
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
                  className="h-8 text-xs"
                  disabled={pending}
                  aria-label={`Transfer project to ${member.email}`}
                >
                  Transfer project
                </Button>
              }
              title={`Hand this project to ${personName(who(member))}?`}
              description="They become its owner when they accept: it moves into an organization they own, with its members and API keys. Its Slack, Discord and webhook connectors stay with your organization and are disconnected. You stay on as an admin of the project, and join their organization as a member. The offer lapses in 7 days if they do nothing."
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
                  className="h-8 text-xs"
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
          {!isOwner && canManage && (
            <ConfirmDialog
              trigger={
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8 text-xs"
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
