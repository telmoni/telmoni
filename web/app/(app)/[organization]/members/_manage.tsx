"use client";

import { UserPlus } from "lucide-react";
import { startTransition, useState } from "react";
import { toast } from "sonner";

import { InviteSentDialog } from "@/components/members/invite-sent-dialog";
import { PageAction } from "@/components/page-action";
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
import type { GivenRole } from "@/lib/roles";
import { OrganizationRole } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { inviteOrganizationMemberAction } from "./actions";

// The invitation takes `organizationId`, the organization the page rendered,
// and hands it to its action, which refuses when its request resolves another
// one: the page has gone stale since.

export function AddOrganizationMember({ organizationId }: { organizationId: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <PageAction primary="Invite member" onClick={() => setOpen(true)}>
        <UserPlus className="size-4" />
        Invite member
      </PageAction>
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
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<GivenRole>(OrganizationRole.Member);
  const [link, setLink] = useState<string | null>(null);
  const { pending, error, run } = useAction();

  function submit() {
    run(
      () => inviteOrganizationMemberAction(organizationId, email, role),
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
              onValueChange={(v) => setRole(v as GivenRole)}
            >
              <SelectTrigger id="organization-member-role" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={OrganizationRole.Admin}>
                  Admin — create projects, manage members & work as an admin in every project
                </SelectItem>
                <SelectItem value={OrganizationRole.Member}>
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
