"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  type CreateOrganizationField,
  createOrganizationAction,
} from "@/app/(app)/actions";
import { MAX_ORGANIZATION_NAME } from "@/lib/organization-name";
import { SLUG_MAX_LENGTH } from "@/lib/slug";

interface Refusal {
  message: string;
  field?: CreateOrganizationField;
}

export function CreateOrganizationDialog({
  open,
  onOpenChange,
  host,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  /// The console's host, shown ahead of the URL as the organization's Settings
  /// show it.
  host: string;
}) {
  const router = useRouter();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = name.trim().length > 0 && !pending;
  const under = (field: CreateOrganizationField) =>
    refusal?.field === field ? refusal.message : null;

  function submit() {
    setRefusal(null);
    start(async () => {
      try {
        const res = await createOrganizationAction(name, slug);
        if (res.error) {
          setRefusal({ message: res.error, field: res.field });
          return;
        }
        onOpenChange(false);
        toast.success("Organization created.");
        // On to its Overview. Without an address to follow, auth's answer
        // could not be read; the switcher lists it once the layout is read
        // again.
        if (res.href) router.push(res.href);
        else router.refresh();
      } catch {
        setRefusal({ message: "Something went wrong. Please try again." });
      }
    });
  }

  const nameError = under("name");
  const urlError = under("slug");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Create an organization</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              An organization holds projects and the people who work on them.
              You will be its owner, and can rename it or change its URL later
              in its settings.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor="organization-name">Name</Label>
              <Input
                id="organization-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Acme"
                maxLength={MAX_ORGANIZATION_NAME}
                autoComplete="off"
                required
                aria-invalid={nameError ? true : undefined}
                aria-describedby={nameError ? "organization-name-error" : undefined}
              />
              {nameError && (
                <p id="organization-name-error" className="text-sm text-destructive" role="alert">
                  {nameError}
                </p>
              )}
            </div>
            <div className="grid gap-2">
              <Label htmlFor="organization-url">URL</Label>
              <div className="flex items-center gap-1">
                <span className="shrink-0 text-sm text-muted-foreground">{host}/</span>
                <Input
                  id="organization-url"
                  value={slug}
                  onChange={(e) => setSlug(e.target.value)}
                  placeholder="acme"
                  maxLength={SLUG_MAX_LENGTH}
                  autoCapitalize="none"
                  autoCorrect="off"
                  autoComplete="off"
                  spellCheck={false}
                  aria-invalid={urlError ? true : undefined}
                  aria-describedby={urlError ? "organization-url-error" : "organization-url-hint"}
                />
              </div>
              {urlError ? (
                <p id="organization-url-error" className="text-sm text-destructive" role="alert">
                  {urlError}
                </p>
              ) : (
                <p id="organization-url-hint" className="text-sm text-muted-foreground">
                  Optional. Left blank, it is made from the name, or is a
                  placeholder until you choose one when the name has no Latin
                  letters or digits.
                </p>
              )}
            </div>
            {refusal && !refusal.field && (
              <p className="text-sm text-destructive" role="alert">
                {refusal.message}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {pending ? "Creating…" : "Create organization"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
