"use client";

import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

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
 * that one or nothing, whatever another tab has switched to since.
 */
export function LeaveOrganization({
  organizationId,
  name,
}: {
  organizationId: string;
  name: string;
}) {
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  return (
    <Card className="gap-3">
      <div className="grid gap-1">
        <p className="text-sm font-medium">Leave {name}</p>
        <p className="text-sm text-muted-foreground">
          You will be removed from this organization and from every project in it.
          Organizations you own are not affected. Its owner can invite you
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
              className="h-8 text-xs"
              disabled={pending}
              aria-label={`Leave ${name}`}
            >
              Leave organization
            </Button>
          }
          title={`Leave ${name}?`}
          description="You will lose access to this organization and every project in it immediately."
          confirmLabel="Leave"
          onConfirm={() =>
            start(async () => {
              setError(null);
              try {
                const r = await leaveOrganizationAction(organizationId);
                // A success redirects and never returns; anything here is a
                // refusal the person can act on.
                if (r?.error) setError(r.error);
              } catch {
                setError("Network error. Try again.");
              }
            })
          }
        />
      </div>
    </Card>
  );
}
