"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";

import { renameOrganizationAction } from "./rename-actions";

export function RenameOrganizationForm({
  organizationId,
  initialName,
  fallbackLabel,
  canEdit,
}: {
  /// The organization this page rendered; the rename refuses any other.
  organizationId: string;
  initialName: string;
  fallbackLabel: string;
  canEdit: boolean;
}) {
  const router = useRouter();
  const [name, setName] = useState(initialName);
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

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
    if (!canEdit || !isChanged || pending) return;
    setError(null);
    start(async () => {
      try {
        const res = await renameOrganizationAction(organizationId, trimmed);
        if (res.error) {
          setError(res.error);
          toast.error(res.error);
          return;
        }
        toast.success("Organization renamed.");
        router.refresh();
      } catch {
        const msg = "Network error. Please try again.";
        setError(msg);
        toast.error(msg);
      }
    });
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          disabled={!canEdit || pending}
          placeholder={fallbackLabel}
          maxLength={MAX_ORGANIZATION_NAME}
          className="max-w-md"
          aria-label="Organization name"
        />
        {canEdit && (
          <Button type="submit" disabled={!isChanged || pending} variant="default">
            {pending ? "Saving..." : "Save"}
          </Button>
        )}
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      {!canEdit && (
        <p className="text-sm text-muted-foreground">
          Only the organization owner can rename it.
        </p>
      )}
    </form>
  );
}
