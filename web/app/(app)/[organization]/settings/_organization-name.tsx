"use client";

import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MAX_ORGANIZATION_NAME, typedBound } from "@/lib/names";
import { useAction } from "@/lib/use-action";

import { renameOrganizationAction } from "./settings-actions";

export function OrganizationNameForm({
  organizationId,
  initialName,
}: {
  /// The organization this page rendered; the rename refuses any other.
  organizationId: string;
  initialName: string;
}) {
  const [name, setName] = useState(initialName);
  const { pending, error, run } = useAction();

  // A rename elsewhere — another tab, or a router.refresh() from the realtime
  // listener — sends a new initialName down. Without this the input keeps the
  // old text while isChanged compares it against the new server value. Only a
  // genuine change resets, so a plain refresh does not discard what is typed.
  const [lastInitial, setLastInitial] = useState(initialName);
  if (initialName !== lastInitial) {
    setLastInitial(initialName);
    setName(initialName);
  }

  const trimmed = name.trim();
  const isChanged = trimmed !== initialName && trimmed.length > 0;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!isChanged || pending) return;
    // A name moves no URL: the page stays where it is, redrawn by the
    // action's revalidation.
    run(
      () => renameOrganizationAction(organizationId, trimmed),
      () => toast.success("Organization renamed."),
    );
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          disabled={pending}
          placeholder="Organization name"
          maxLength={typedBound(MAX_ORGANIZATION_NAME)}
          className="max-w-md"
          aria-label="Organization name"
        />
        <Button type="submit" disabled={!isChanged || pending} variant="default">
          {pending ? "Saving..." : "Save"}
        </Button>
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
    </form>
  );
}
