"use client";

import { useState, useTransition } from "react";

import { LocalTime } from "@/components/local-time";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { organizationLabel } from "@/lib/identity";
import type { DeletedOrganization } from "@/lib/server/data";

import { restoreOrganizationAction } from "./actions";

/// The organizations this person owns that are on their way out, each with
/// the day its window closes and, while the person may still change their
/// mind, a Restore. One Telmoni closed is listed too, with support named in
/// place of the button, so the page never shows a deletion the person did
/// not ask for as theirs to undo.
///
/// The label is the organization's name, as everywhere in the console. One
/// closed before its owner named it — an operator's termination, since the
/// console opens to nobody until it is named — reads as "Organization".
export function DeletedOrganizations({
  organizations,
}: {
  organizations: readonly DeletedOrganization[];
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, startTransition] = useTransition();

  if (organizations.length === 0) return null;

  function handleRestore(organizationId: string) {
    setError(null);
    setBusy(organizationId);
    startTransition(async () => {
      try {
        const r = await restoreOrganizationAction(organizationId);
        if (r.error) {
          setError(r.error);
          setBusy(null);
          return;
        }
        // A full navigation, not a router push: the rail, the selector and
        // the store were all read without the organization that is back.
        window.location.replace("/console");
      } catch {
        setError("Network error. Try again.");
        setBusy(null);
      }
    });
  }

  return (
    <Card className="grid gap-4 text-sm" data-testid="deleted-organizations">
      <ul className="grid gap-3">
        {organizations.map((o) => (
          <li
            key={o.organizationId}
            className="flex flex-wrap items-center justify-between gap-3"
          >
            <div className="grid gap-1">
              <span className="font-medium">{organizationLabel({ name: o.name })}</span>
              <span className="text-muted-foreground">
                {o.restorable ? (
                  <>
                    Deleted on <LocalTime iso={o.deletionRequestedAt} mode="date" />. You can
                    restore it until <LocalTime iso={o.eraseAfter} mode="date" />; after that it
                    is erased.
                  </>
                ) : (
                  <>
                    Closed on <LocalTime iso={o.deletionRequestedAt} mode="date" />, and erased
                    after <LocalTime iso={o.eraseAfter} mode="date" />. Contact support to have
                    it restored.
                  </>
                )}
              </span>
            </div>
            {o.restorable && (
              <Button
                size="sm"
                variant="outline"
                disabled={busy !== null}
                onClick={() => handleRestore(o.organizationId)}
              >
                {busy === o.organizationId ? "Restoring…" : "Restore"}
              </Button>
            )}
          </li>
        ))}
      </ul>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </Card>
  );
}
