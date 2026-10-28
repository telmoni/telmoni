"use client";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useAction } from "@/lib/use-action";

import { leaveOrganizationAction } from "./members/actions";

/**
 * The door out of an organization somebody else owns.
 *
 * Rendered only when the caller is not the active organization's owner — an
 * owner hands the organization to an admin first, and auth refuses the owner
 * leaving too.
 *
 * ⚠ **This is not "leave a project".** Leaving a PROJECT gives up one seat and keeps
 * the roster row, which is the right behaviour: an owner can seat you again
 * tomorrow. Leaving the ORGANIZATION takes the roster row and every project seat
 * under it, so the description has to say so — somebody who reads it as the
 * project button will lose more than they meant to.
 *
 * `organizationId` is the organization the page rendered: the action leaves
 * that one or nothing, whichever one its request resolves once this page has
 * gone stale (`SWITCHED_ORGANIZATION`).
 */
export function LeaveOrganization({
  organizationId,
  name,
}: {
  organizationId: string;
  name: string;
}) {
  const { pending, error, run } = useAction();

  return (
    <Card className="gap-3">
      <div className="grid gap-1">
        <p className="text-sm font-medium">Leave {name}</p>
        <p className="text-sm text-muted-foreground">
          You will be removed from this organization and from every project in it.
          Organizations you own are not affected. An owner or admin can invite you
          back.
        </p>
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <div>
        <ConfirmDialog
          trigger={
            <Button
              variant="outline"
              size="sm"
              className="h-9 text-xs"
              disabled={pending}
            >
              {/* Named by what it shows, which a voice command says. */}
              Leave organization<span className="sr-only">: {name}</span>
            </Button>
          }
          title={`Leave ${name}?`}
          description="You will lose access to this organization and every project in it immediately."
          confirmLabel="Leave"
          // A success redirects; what comes back is a refusal to say.
          onConfirm={() => run(() => leaveOrganizationAction(organizationId))}
        />
      </div>
    </Card>
  );
}
