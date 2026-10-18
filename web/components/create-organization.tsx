"use client";

import { unstable_rethrow, useRouter } from "next/navigation";
import { Building2 } from "lucide-react";
import { startTransition, useId, useRef, useState, useTransition } from "react";
import { toast } from "sonner";

import {
  SidePanel,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

/** New organization, in the console's side panel (`create-panels.tsx`). */
export function CreateOrganizationPanel({
  host,
  notice,
  onClose,
  returnFocus,
}: {
  /// The console's host, shown ahead of the URL as the organization's Settings
  /// show it.
  host: string;
  /// Drawn first in the panel: what a console built on this one says a new
  /// organization starts on there, and any fields of its own it asks for,
  /// which the action is handed by name.
  notice?: React.ReactNode;
  onClose: () => void;
  returnFocus?: () => HTMLElement | null;
}) {
  const router = useRouter();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [refusal, setRefusal] = useState<Refusal | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = name.trim().length > 0 && !pending;
  const under = (field: CreateOrganizationField) =>
    refusal?.field === field ? refusal.message : null;

  function submit(extras: Record<string, string>) {
    setRefusal(null);
    start(async () => {
      try {
        const res = await createOrganizationAction(name, slug, extras);
        if (res.error) {
          setRefusal({ message: res.error, field: res.field });
          return;
        }
        // A console built on this one may send the person off this console
        // first — to pay for it — and founds it itself after: the page is
        // left for that address, and nothing is created yet to say so.
        if (res.href && /^https?:\/\//.test(res.href)) {
          window.location.assign(res.href);
          return;
        }
        // On to its Overview. Without an address to follow, auth's answer
        // could not be read; the switcher lists it all the same, the layout
        // read again by the action's revalidation. The panel shuts in one
        // transition with either, so it goes as the page lands.
        startTransition(() => {
          onClose();
          if (res.href) router.push(res.href);
        });
        toast.success("Organization created.");
      } catch (err) {
        unstable_rethrow(err);
        setRefusal({ message: "Something went wrong. Please try again." });
      }
    });
  }

  const nameError = under("name");
  const urlError = under("slug");

  return (
    <SidePanel
      panel={panel}
      titleId={titleId}
      onClose={onClose}
      full
      initialFocus={field}
      returnFocus={returnFocus}
    >
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (!canSubmit) return;
          // What the notice's own fields say, by name: the name and URL
          // fields carry none, so they are not among them.
          const extras: Record<string, string> = {};
          for (const [key, value] of new FormData(e.currentTarget).entries()) {
            if (typeof value === "string") extras[key] = value;
          }
          submit(extras);
        }}
      >
        <SidePanelHeader
          icon={Building2}
          titleId={titleId}
          title="New organization"
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            An organization holds projects and the people who work on them.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-3.5 py-2">
          {notice}
          <SidePanelRow label="Name" htmlFor="organization-name">
            <Input
              ref={field}
              id="organization-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Acme"
              maxLength={MAX_ORGANIZATION_NAME}
              autoComplete="off"
              required
              aria-invalid={nameError ? true : undefined}
              aria-describedby={nameError ? "organization-name-error" : "organization-name-hint"}
              className="max-w-md"
            />
            {nameError ? (
              <p id="organization-name-error" className="text-destructive" role="alert">
                {nameError}
              </p>
            ) : (
              <p id="organization-name-hint" className="text-muted-foreground">
                What everyone in it sees. You can rename it later in its settings.
              </p>
            )}
          </SidePanelRow>
          <SidePanelRow label="URL" htmlFor="organization-url">
            <div className="flex max-w-md items-center gap-1">
              <span className="shrink-0 text-muted-foreground">{host}/</span>
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
              <p id="organization-url-error" className="text-destructive" role="alert">
                {urlError}
              </p>
            ) : (
              <p id="organization-url-hint" className="text-muted-foreground">
                Optional. Left blank, it is made from the name, or is a placeholder until you
                choose one when the name has no Latin letters or digits. You can change it later
                in its settings.
              </p>
            )}
          </SidePanelRow>
          <SidePanelRow label="Your role">
            <span>Owner, with every right in it.</span>
          </SidePanelRow>
          {refusal && !refusal.field && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {refusal.message}
            </p>
          )}
        </div>
        <SidePanelFooter>
          <Button type="submit" disabled={!canSubmit}>
            {pending ? "Creating…" : "Create organization"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
