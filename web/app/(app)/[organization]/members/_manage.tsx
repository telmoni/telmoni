"use client";

import { UserPlus } from "lucide-react";
import { startTransition, useId, useRef, useState } from "react";
import { toast } from "sonner";

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
import type { GivenRole } from "@/lib/roles";
import { OrganizationRole } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { inviteOrganizationMemberAction } from "./actions";

// The invitation takes `organizationId`, the organization the page rendered,
// and hands it to its action, which refuses when its request resolves another
// one: the page has gone stale since.

export function AddOrganizationMember({ organizationId }: { organizationId: string }) {
  const { opening, open, close, place } = usePanelOpening();
  const [sent, setSent] = useState<{ email: string; link: string } | null>(null);
  return (
    <>
      <PageAction primary="Invite member" onClick={(e) => open(e.currentTarget, null)}>
        <UserPlus className="size-icon" />
        Invite member
      </PageAction>
      {place(
        opening && (
          <InvitePanel
            key={opening.at}
            organizationId={organizationId}
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
          scope="organization"
        />
      )}
    </>
  );
}

function InvitePanel({
  organizationId,
  onClose,
  returnFocus,
  onInvited,
}: {
  organizationId: string;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
  /** An invitation sent, with the link it carries where there is one. */
  onInvited: (email: string, link: string | null) => void;
}) {
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<GivenRole>(OrganizationRole.Member);
  const { pending, error, run } = useAction();
  const ready = !pending && email.trim().length > 0;

  function submit() {
    run(
      () => inviteOrganizationMemberAction(organizationId, email, role),
      (r) => {
        // In the transition, with the invitations the action drew again.
        startTransition(() => onInvited(email, r.link ?? null));
        toast.success(`Invitation sent to ${email}`);
      },
    );
  }

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full initialFocus={field} returnFocus={returnFocus}>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) submit();
        }}
      >
        <SidePanelHeader icon={UserPlus} titleId={titleId} title="Invite an organization member" onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            Invite a colleague to this organization. They get a link and can join as an
            admin or a member.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          <SidePanelRow label="Email address" htmlFor="organization-member-email">
            <Input
              ref={field}
              id="organization-member-email"
              type="email"
              autoComplete="off"
              className="max-w-md"
              value={email}
              placeholder="alex@example.com"
              onChange={(e) => setEmail(e.target.value)}
            />
          </SidePanelRow>
          <SidePanelRow label="Organization role" htmlFor="organization-member-role">
            <EnumSelect
              id="organization-member-role"
              value={role}
              options={[OrganizationRole.Admin, OrganizationRole.Member]}
              label={(given) =>
                given === OrganizationRole.Admin
                  ? "Admin — create projects, manage members & work as an admin in every project"
                  : "Member — project-level access only; no organization-wide powers"
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
            {pending ? "Sending…" : "Send invitation"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
