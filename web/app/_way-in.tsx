"use client";

import { useEffect } from "react";

import { PaperLink } from "@/components/paper-page";
import { Button } from "@/components/ui/button";
import { inFront, pressedKey, typedInto } from "@/lib/keys";
import { letterKeysOn, useLetterKeys } from "@/lib/use-accessibility";

// The way in: the console for a member, sign-up for a visitor, and the docs
// for either. The first's key is the header's (`header-shortcuts.tsx`: S to
// sign up, C for the console); D for the docs is this page's. No button draws
// its key; each announces it. Off with every letter key, in Accessibility.

const DOCS_KEY = "d";

export function WayIn({ signedIn, docsHref }: { signedIn: boolean; docsHref: string }) {
  const letterKeys = useLetterKeys();
  const start = signedIn ? { href: "/console", label: "Console" } : { href: "/auth/signup", label: "Sign up" };

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey || e.repeat || typedInto(e)) return;
      if (inFront() || !letterKeysOn() || pressedKey(e).toLowerCase() !== DOCS_KEY) return;
      e.preventDefault();
      // A whole navigation, as the link's: the book may stand on another origin.
      window.location.assign(docsHref);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [docsHref]);

  return (
    <div className="flex flex-wrap items-center gap-gutter">
      <Button asChild size="lg" className="gap-2 px-5">
        {/* Sign-up is a route handler, which only a whole load reaches. */}
        <a href={start.href}>{start.label}</a>
      </Button>
      <Button asChild size="lg" variant="outline" className="gap-2 px-5">
        <PaperLink href={docsHref} aria-keyshortcuts={letterKeys ? DOCS_KEY : undefined}>
          Documentation
        </PaperLink>
      </Button>
    </div>
  );
}
