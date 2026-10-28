"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SLUG_MAX_LENGTH } from "@/lib/slug";
import { useAction } from "@/lib/use-action";

import { changeOrganizationUrlAction } from "./settings-actions";

export function OrganizationUrlForm({
  organizationId,
  slug,
  host,
}: {
  /// The organization this page rendered; the change refuses any other.
  organizationId: string;
  /// The slug its paths begin with now.
  slug: string;
  /// The console's host, shown ahead of the field.
  host: string;
}) {
  const [value, setValue] = useState(slug);
  const { pending, error, run } = useAction();

  const trimmed = value.trim();
  const isChanged = trimmed !== slug && trimmed.length > 0;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!isChanged || pending) return;
    // A change that moved the URL never answers here: the action takes the
    // page on to the new address itself. One that moved nothing is drawn again
    // by the action's revalidation.
    run(() => changeOrganizationUrlAction(organizationId, trimmed));
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <div className="flex max-w-md flex-1 items-center gap-1">
          <span className="shrink-0 text-sm text-muted-foreground">{host}/</span>
          <Input
            value={value}
            onChange={(e) => setValue(e.target.value)}
            disabled={pending}
            placeholder="acme"
            maxLength={SLUG_MAX_LENGTH}
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            aria-label="Organization URL"
          />
        </div>
        <Button type="submit" disabled={!isChanged || pending} variant="default">
          {pending ? "Saving..." : "Save"}
        </Button>
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
    </form>
  );
}
