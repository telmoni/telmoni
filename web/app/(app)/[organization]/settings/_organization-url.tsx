"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

import { SLUG_MAX_LENGTH, organizationPath } from "@/lib/slug";

import { changeOrganizationUrlAction } from "./settings-actions";

export function OrganizationUrlForm({
  organizationId,
  slug,
  host,
  canEdit,
}: {
  /// The organization this page rendered; the change refuses any other.
  organizationId: string;
  /// The slug its paths begin with now.
  slug: string;
  /// The console's host, shown ahead of the field as Vercel shows its own.
  host: string;
  canEdit: boolean;
}) {
  const router = useRouter();
  const [value, setValue] = useState(slug);
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  // A change elsewhere — another tab, or a router.refresh() from the realtime
  // listener — sends a new slug down; only a genuine change resets the field.
  const [lastSlug, setLastSlug] = useState(slug);
  if (slug !== lastSlug) {
    setLastSlug(slug);
    setValue(slug);
  }

  const trimmed = value.trim();
  const isChanged = trimmed !== slug && trimmed.length > 0;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit || !isChanged || pending) return;
    setError(null);
    start(async () => {
      try {
        const res = await changeOrganizationUrlAction(organizationId, trimmed);
        if (res.error) {
          setError(res.error);
          toast.error(res.error);
          return;
        }
        toast.success("URL changed.");
        // The organization's pages, this one included, are at the new address.
        if (res.movedTo) router.replace(organizationPath(res.movedTo, "/settings"));
        else router.refresh();
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
        <div className="flex max-w-md flex-1 items-center gap-1">
          <span className="shrink-0 text-sm text-muted-foreground">{host}/</span>
          <Input
            value={value}
            onChange={(e) => setValue(e.target.value)}
            disabled={!canEdit || pending}
            placeholder="acme"
            maxLength={SLUG_MAX_LENGTH}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            aria-label="Organization URL"
          />
        </div>
        {canEdit && (
          <Button type="submit" disabled={!isChanged || pending} variant="default">
            {pending ? "Saving..." : "Save"}
          </Button>
        )}
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      {!canEdit && (
        <p className="text-sm text-muted-foreground">
          Only an organization owner or admin can change its URL.
        </p>
      )}
    </form>
  );
}
