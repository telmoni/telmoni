"use client";

import { UserPlus } from "lucide-react";
import { startTransition, useId, useRef, useState } from "react";
import { toast } from "sonner";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { EnumSelect } from "@/components/enum-select";
import { InviteSentDialog } from "@/components/members/invite-sent-dialog";
import { PageAction } from "@/components/page-action";
import {
  SidePanel,
  SidePanelBody,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
  usePanelOpening,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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
  const { opening, open, close, place } = usePanelOpening();
  const [sent, setSent] = useState<{ email: string; link: string } | null>(null);
  return (
    <>
      <PageAction primary="Add member" onClick={(e) => open(e.currentTarget, null)}>
        <UserPlus className="size-icon" />
        Add member
      </PageAction>
      {place(
        opening && (
          <AddPanel
            key={opening.at}
            projectId={projectId}
            candidates={candidates}
            rosterUnread={rosterUnread}
            inviteClosed={inviteClosed}
            onClose={close}
            returnFocus={() => opening.returnTo}
            onInvited={(email, link) => {
              close();
              if (link) setSent({ email, link });
            }}
          />
        ),
      )}
      {sent && (
        <InviteSentDialog
          open
          onOpenChange={(next) => {
            if (!next) setSent(null);
          }}
          email={sent.email}
          link={sent.link}
          scope="project"
        />
      )}
    </>
  );
}

// Two ways in. A member of the organization is seated at once, with nothing
// to accept — offered while there is one left to seat, to the organization's
// owner and admins, who alone are handed its roster. Anybody else is invited
// by email, as before. One role serves both.
function AddPanel({
  projectId,
  candidates,
  rosterUnread,
  inviteClosed,
  onClose,
  returnFocus,
  onInvited,
}: {
  projectId: string;
  candidates: readonly Candidate[];
  rosterUnread: boolean;
  inviteClosed: string | undefined;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
  /** An invitation sent, with the link it carries where there is one. */
  onInvited: (email: string, link: string | null) => void;
}) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const [way, setWay] = useState<Way>(candidates.length > 0 ? "roster" : "email");
  // Nobody chosen until somebody is: a seat given mails its person.
  const [memberId, setMemberId] = useState("");
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<GivenRole>(Role.Member);
  const { pending, error, setError, run } = useAction();
  const chosen = candidates.find((c) => c.memberId === memberId);
  const fromRoster = way === "roster" && candidates.length > 0;
  const ready = !pending && (fromRoster ? chosen !== undefined : email.trim().length > 0);

  function submit() {
    if (fromRoster) {
      if (!chosen) return;
      run(
        () => addMemberAction(projectId, chosen.memberId, role),
        () => {
          toast.success(
            `Added ${personName(chosen)} as ${role === Role.Admin ? "an admin" : "a member"}.`,
          );
          startTransition(onClose);
        },
      );
      return;
    }
    run(
      () => inviteMemberAction(projectId, email, role),
      (r) => {
        // In the transition, with the invitations the action drew again.
        startTransition(() => onInvited(email, r.link ?? null));
        toast.success(`Invitation sent to ${email}`);
      },
    );
  }

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full returnFocus={returnFocus}>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) submit();
        }}
      >
        <SidePanelHeader icon={UserPlus} titleId={titleId} title="Add a member" onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            {fromRoster
              ? "Somebody already in the organization is added at once, with nothing to accept, and told by email and in their notifications."
              : "By email address. They get a link, and nothing changes here until they accept it — whether or not they already have an organization."}
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          {candidates.length > 0 && (
            <SidePanelRow label="Who" labelId="member-way">
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
              {inviteClosed && (
                <p id="member-way-closed" className="text-muted-foreground" role="note">
                  {inviteClosed}
                </p>
              )}
            </SidePanelRow>
          )}
          {fromRoster ? (
            <SidePanelRow label="Person" htmlFor="member-person">
              <Select value={memberId} onValueChange={setMemberId}>
                <SelectTrigger id="member-person" className="w-full max-w-md">
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
            </SidePanelRow>
          ) : (
            <SidePanelRow label="Email address" htmlFor="member-email">
              <Input
                id="member-email"
                type="email"
                autoComplete="off"
                className="max-w-md"
                value={email}
                placeholder="dana@example.com"
                onChange={(e) => setEmail(e.target.value)}
              />
              {rosterUnread && (
                <p className="text-muted-foreground" role="note">
                  The organization&rsquo;s members can&rsquo;t be read right now, so
                  adding one of them waits; reload in a moment.
                </p>
              )}
            </SidePanelRow>
          )}
          <SidePanelRow label="Role" htmlFor="member-role">
            <EnumSelect
              id="member-role"
              value={role}
              options={[Role.Member, Role.Admin]}
              label={(given) =>
                given === Role.Admin ? "Admin — read access and the audit log" : "Member — read-only access"
              }
              onChange={setRole}
              className="w-full max-w-md"
            />
          </SidePanelRow>
          {error && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" variant="filled" disabled={!ready}>
            {fromRoster
              ? pending
                ? "Adding…"
                : "Add"
              : pending
                ? "Sending…"
                : "Send invitation"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
