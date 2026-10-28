"use client";

import { UserPlus } from "lucide-react";
import { startTransition, useState } from "react";
import { toast } from "sonner";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { EnumSelect } from "@/components/enum-select";
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
import { personDetail, personName } from "@/lib/identity";
import type { GivenRole } from "@/lib/roles";
import { Role } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { addMemberAction, inviteMemberAction } from "./actions";

/** A member of the organization holding no seat on the project yet: the
 *  owner and admins may seat them here with nothing to accept. */
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
            <EnumSelect
              id="member-role"
              value={role}
              options={[Role.Member, Role.Admin]}
              label={(given) =>
                given === Role.Admin ? "Admin — read access and the audit log" : "Member — read-only access"
              }
              onChange={setRole}
              className="w-full"
            />
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
