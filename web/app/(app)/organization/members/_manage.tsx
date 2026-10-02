"use client";

import { InviteSentDialog } from "@/components/members/invite-sent-dialog";

import { useRouter } from "next/navigation";
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
import type { OrganizationInvite, OrganizationMember } from "@/lib/server/data";
import { roleLabel } from "@/lib/role-label";

import {
  cancelOwnershipOfferAction,
  inviteOrganizationMemberAction,
  offerOwnershipAction,
  revokeOrganizationInviteAction,
  removeOrganizationMemberAction,
  updateOrganizationMemberRoleAction,
} from "./actions";

type GrantableOrganizationRole = "admin" | "member";

// Every control here takes `organizationId`, the organization the page
// rendered, and hands it to its action, which refuses when another tab has
// switched the console elsewhere since.

export function AddOrganizationMember({ organizationId }: { organizationId: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <PageAction primary="Invite member" onClick={() => setOpen(true)}><UserPlus className="size-4" />Invite member</PageAction>
      <AddOrganizationMemberDialog
        key={`add-organization-member-${open}`}
        organizationId={organizationId}
        open={open}
        onOpenChange={setOpen}
      />
    </>
  );
}

function AddOrganizationMemberDialog({
  organizationId,
  open,
  onOpenChange,
}: {
  organizationId: string;
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<GrantableOrganizationRole>("member");
  const [error, setError] = useState<string | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const [pending, start] = useTransition();

  function submit() {
    setError(null);
    start(async () => {
      try {
        const r = await inviteOrganizationMemberAction(organizationId, email, role);
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
        scope="organization"
      />
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Invite an organization member</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription>
            Invite a colleague to this organization. They get a link and can join as an
            admin or a member.
          </DialogDescription>
          <div className="grid gap-1.5">
            <Label htmlFor="organization-member-email">Email address</Label>
            <Input
              id="organization-member-email"
              type="email"
              autoComplete="off"
              value={email}
              placeholder="alex@example.com"
              onChange={(e) => setEmail(e.target.value)}
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="organization-member-role">Organization role</Label>
            <Select
              value={role}
              onValueChange={(v) => setRole(v as GrantableOrganizationRole)}
            >
              <SelectTrigger id="organization-member-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="admin">
                  Admin — create projects, manage members & work as an admin in every project
                </SelectItem>
                <SelectItem value="member">
                  Member — project-level access only; no organization-wide powers
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

export function OrganizationMemberRow({
  organizationId,
  organizationNamed,
  member,
  canManage,
  isOwnerCaller = false,
}: {
  organizationId: string;
  organizationNamed: boolean;
  member: OrganizationMember;
  canManage: boolean;
  isOwnerCaller?: boolean;
}) {
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
  // Only an admin can be handed the organization, and only the owner hands it.
  const canOffer = isOwnerCaller && member.role === "admin";
  const offered = Boolean(member.ownership_offer_expires_at);
  const transferTrigger = (
    <Button
      variant="outline"
      size="sm"
      className="h-8 text-xs"
      disabled={pending}
      aria-label={`Transfer ownership to ${member.email}`}
    >
      Transfer ownership
    </Button>
  );

  return (
    <tr>
      <Td className="font-medium">
        <div className="flex items-center gap-2">
          <span>{personName(who(member))}</span>
          {isOwner && (
            <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
              Organization Owner
            </Badge>
          )}
          {offered && member.ownership_offer_expires_at && (
            <Badge variant="outline" className="text-[10px] px-1.5 py-0">
              Ownership offered · lapses{" "}
              <LocalTime iso={member.ownership_offer_expires_at} mode="date" />
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
              act(() =>
                updateOrganizationMemberRoleAction(
                  organizationId,
                  member.member_id,
                  v as GrantableOrganizationRole,
                ),
              )
            }
          >
            <SelectTrigger
              className="h-8 w-32 text-xs"
              aria-label={`Role for ${member.email}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="admin">Admin</SelectItem>
              <SelectItem value="member">Member</SelectItem>
            </SelectContent>
          </Select>
        ) : (
          <span className="text-xs text-muted-foreground">
            {roleLabel(member.role)}
          </span>
        )}
        {error && <span className="ml-2 text-xs text-destructive">{error}</span>}
      </Td>
      <Td className="text-xs text-muted-foreground whitespace-nowrap">
        <LocalTime iso={member.created_at} mode="date" />
      </Td>
      <Td className="text-right whitespace-nowrap">
        <div className="flex items-center justify-end gap-3">
          {canOffer && !offered && organizationNamed && (
            <ConfirmDialog
              trigger={transferTrigger}
              title={`Hand this organization to ${personName(who(member))}?`}
              description="They become the owner when they accept, and its API keys, members and deleting the organization become theirs. You stay on as an admin: an admin in every project, with no say over its members, keys or connectors. The offer lapses in 7 days if they do nothing."
              confirmLabel="Send offer"
              onConfirm={() =>
                act(() => offerOwnershipAction(organizationId, member.member_id))
              }
            />
          )}
          {canOffer && !offered && !organizationNamed && (
            <ConfirmDialog
              trigger={transferTrigger}
              title="Name this organization first"
              description="Until it has a name, this organization is shown by its owner's email address, so handing it over would show it by the new owner's address instead, for everyone in it. Name it in the organization's settings, then hand it over."
              confirmLabel="Go to settings"
              onConfirm={() => router.push("/organization/settings")}
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
                  aria-label={`Withdraw the ownership offer to ${member.email}`}
                >
                  Withdraw offer
                </Button>
              }
              title="Withdraw the ownership offer?"
              description="The organization stays yours, and they can no longer accept it."
              confirmLabel="Withdraw"
              onConfirm={() => act(() => cancelOwnershipOfferAction(organizationId))}
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
              title={`Remove ${personName(who(member))} from organization?`}
              description="They will immediately lose access to all projects and resources in this organization."
              confirmLabel="Remove"
              onConfirm={() =>
                act(() => removeOrganizationMemberAction(organizationId, member.member_id, member.email))
              }
            />
          )}
        </div>
      </Td>
    </tr>
  );
}

export function OrganizationInviteRow({
  organizationId,
  invite,
  canManage,
}: {
  organizationId: string;
  invite: OrganizationInvite;
  canManage: boolean;
}) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

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
                  const r = await revokeOrganizationInviteAction(organizationId, invite.id);
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

function who(member: OrganizationMember): {
  displayName?: string | null;
  email: string;
} {
  return { displayName: member.display_name, email: member.email };
}
