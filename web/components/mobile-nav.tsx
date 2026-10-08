"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { Menu, MessagesSquare, X } from "lucide-react";

import { GithubIcon } from "@/components/brand-icons";
import { Button } from "@/components/ui/button";
import { PRIMARY_NAV, panelLinks, type NavLink } from "@/components/site-nav";
import { DISCUSSIONS_URL, REPO_URL } from "@/lib/site";

const NAV_TYPE = "text-sm font-normal w-full justify-start";

export function MobileNav({ signedIn }: { signedIn: boolean }) {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;

    const handlePointerDown = (event: MouseEvent | TouchEvent) => {
      const target = event.target as Node;
      if (menuRef.current?.contains(target)) return;
      if (triggerRef.current?.contains(target)) return;
      setOpen(false);
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  return (
    <>
      <Button
        ref={triggerRef}
        variant="ghost"
        size={null}
        className="size-8"
        aria-label={open ? "Close menu" : "Open menu"}
        onClick={() => setOpen(!open)}
      >
        {open ? <X className="size-4" /> : <Menu className="size-4" />}
      </Button>

      {open && (
        <div
          data-slot="mobile-nav-backdrop"
          className="fixed inset-x-0 top-15 bottom-0 z-50 overflow-y-auto bg-background/80 backdrop-blur-sm"
          onClick={() => setOpen(false)}
        >
          <div
            ref={menuRef}
            className="flex w-full flex-col border-b bg-background px-4 pb-6 pt-4"
            onClick={(e) => e.stopPropagation()}
          >
            <nav aria-label="Primary" className="flex flex-col gap-1 px-2">
              {/* ⚠ **No panels down here, and not for want of room.** A panel
                  opens on hover over a label that goes nowhere, and neither
                  half of that survives a touch screen — the hover does not
                  exist and the label would swallow a tap that meant to
                  navigate. So a panel's label becomes a heading and the links
                  it would have opened are listed flat beneath it. */}
              {PRIMARY_NAV.map((item) =>
                item.panel ? (
                  <div key={item.label} className="flex flex-col gap-1">
                    <span className="px-3 pt-3 pb-1 text-xs tracking-label text-muted-foreground uppercase">
                      {item.label}
                    </span>
                    {panelLinks(item.panel).map((link) => (
                      <Row key={link.href} link={link} onDone={() => setOpen(false)} />
                    ))}
                  </div>
                ) : (
                  <Row key={item.href} link={item} onDone={() => setOpen(false)} />
                ),
              )}

              <span aria-hidden className="my-2 h-px bg-border" />

              {/* The header's pair, stacked, at the gap every button row uses: a
                  visitor's way out outlined and the thing to do filled; a
                  member's both outlined, as the header has them. */}
              {signedIn ? (
                <div className="flex flex-col gap-3">
                  <Button asChild size="sm" variant="outline" className={NAV_TYPE}>
                    <Link href="/console" onClick={() => setOpen(false)}>
                      Console
                    </Link>
                  </Button>
                  <Button asChild size="sm" variant="outline" className={NAV_TYPE}>
                    {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                    <a href="/auth/logout" onClick={() => setOpen(false)}>
                      Sign out
                    </a>
                  </Button>
                </div>
              ) : (
                <div className="flex flex-col gap-3">
                  <Button asChild size="sm" variant="outline" className={NAV_TYPE}>
                    {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                    <a href="/auth/login">
                      Sign in
                    </a>
                  </Button>
                  <Button asChild size="sm" className={NAV_TYPE}>
                    {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                    <a href="/auth/signup">
                      Sign up
                    </a>
                  </Button>
                </div>
              )}

              {(REPO_URL || DISCUSSIONS_URL) && (
                <span aria-hidden className="my-2 h-px bg-border" />
              )}

              {REPO_URL && (
                <Button asChild variant="ghost" size="sm" className={`${NAV_TYPE} text-muted-foreground hover:text-foreground`}>
                  <a
                    href={REPO_URL}
                    target="_blank"
                    rel="noreferrer"
                    onClick={() => setOpen(false)}
                  >
                    <GithubIcon />
                    GitHub
                  </a>
                </Button>
              )}
              {DISCUSSIONS_URL && (
                <Button asChild variant="ghost" size="sm" className={`${NAV_TYPE} text-muted-foreground hover:text-foreground`}>
                  <a
                    href={DISCUSSIONS_URL}
                    target="_blank"
                    rel="noreferrer"
                    onClick={() => setOpen(false)}
                  >
                    <MessagesSquare className="size-4" />
                    Discussions
                  </a>
                </Button>
              )}
            </nav>
          </div>
        </div>
      )}
    </>
  );
}

function Row({ link, onDone }: { link: NavLink; onDone: () => void }) {
  return (
    <Button
      asChild
      variant="ghost"
      size="sm"
      className={`${NAV_TYPE} text-muted-foreground hover:text-foreground`}
    >
      <Link
        href={link.href}
        {...(link.newTab && { target: "_blank", rel: "noreferrer" })}
        onClick={onDone}
      >
        {link.label}
      </Link>
    </Button>
  );
}
