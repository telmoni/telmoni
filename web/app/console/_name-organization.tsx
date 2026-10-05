"use client";

import { useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";

import { organizationPath } from "@/lib/slug";

import { nameOrganizationAction } from "@/app/(app)/[organization]/settings/settings-actions";

// The first name an organization gets, before the console opens to its owner.
// The first name is what takes the organization off its placeholder slug, and
// auth answers the URL it chose.
export function NameOrganizationForm({ organizationId }: { organizationId: string }) {
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const trimmed = name.trim();

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!trimmed || pending) return;
    setError(null);
    start(async () => {
      try {
        const res = await nameOrganizationAction(organizationId, trimmed);
        if (res.error) {
          setError(res.error);
          return;
        }
        // A full navigation, not a router push: the rail, the selector and the
        // store were all seeded without the name. Straight to the overview at
        // the slug the organization goes by now, moved by the name or the
        // placeholder it kept — `/console` would ask `/me` for the cookie's
        // organization, which is another one when the owner came here from it.
        window.location.replace(res.slug ? organizationPath(res.slug) : "/console");
      } catch {
        setError("Network error. Please try again.");
      }
    });
  };

  return (
    <form onSubmit={handleSubmit} className="mt-4 grid gap-3">
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        disabled={pending}
        placeholder="Acme"
        maxLength={MAX_ORGANIZATION_NAME}
        aria-label="Organization name"
        autoFocus
      />
      {error && <p className="text-sm text-destructive">{error}</p>}
      <div>
        <Button type="submit" disabled={!trimmed || pending}>
          {pending ? "Saving..." : "Continue"}
        </Button>
      </div>
    </form>
  );
}
