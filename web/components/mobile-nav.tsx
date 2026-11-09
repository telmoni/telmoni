"use client";

import { useEffect, useRef, useState } from "react";
import { Menu, X } from "lucide-react";

import { RailLink } from "@/components/rail-link";
import { SignOutButton } from "@/components/sign-out-button";
import { Button } from "@/components/ui/button";
import { PRIMARY_NAV, panelLinks } from "@/components/site-nav";
import { INSET } from "@/lib/inset";
import { DISCUSSIONS_URL } from "@/lib/site";
import { cn } from "@/lib/utils";

// A row of the menu: a touch's height, the rail's spacing between lines.
const ROW = "py-1.5";

export function MobileNav({ signedIn }: { signedIn: boolean }) {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
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
    // The page under the sheet holds still while it is open: a scroll there
    // moved what the sheet covers and nothing the eye can see.
    const root = document.documentElement;
    const overflow = root.style.overflow;
    root.style.overflow = "hidden";

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
      root.style.overflow = overflow;
    };
  }, [open]);

  return (
    <>
      <Button
        ref={triggerRef}
        variant="ghost"
        size="icon"
        className="text-muted-foreground hover:text-foreground"
        aria-label={open ? "Close menu" : "Open menu"}
        onClick={() => setOpen(!open)}
      >
        {open ? <X className="size-icon" /> : <Menu className="size-icon" />}
      </Button>

      {open && (
        <div
          data-slot="mobile-nav-backdrop"
          className="fixed inset-x-0 top-15 bottom-0 z-50 overflow-y-auto overscroll-contain bg-background/80 backdrop-blur-sm"
          onClick={() => setOpen(false)}
        >
          <div
            ref={menuRef}
            className={cn("flex w-full flex-col border-b bg-background pt-4 pb-page", INSET)}
            onClick={(e) => e.stopPropagation()}
          >
            {/* The desktop rail's list, as it draws it: plain links in one
                column, the account's last, a new tab's arrow always shown,
                since a touch has no hover to show it on. */}
            <nav aria-label="Primary" className="flex flex-col text-sm">
              {/* ⚠ **No panels down here, and not for want of room.** A panel
                  opens on hover over a label that goes nowhere, and neither
                  half of that survives a touch screen — the hover does not
                  exist and the label would swallow a tap that meant to
                  navigate. So a panel's label becomes a heading and the links
                  it would have opened are listed flat beneath it. */}
              {PRIMARY_NAV.map((item) =>
                item.panel ? (
                  <div key={item.label} className="flex flex-col">
                    <span className="pt-gutter pb-1 text-xs tracking-label text-muted-foreground uppercase">
                      {item.label}
                    </span>
                    {panelLinks(item.panel).map((link) => (
                      <RailLink key={link.href} link={link} arrow="always" className={ROW} onDone={close} />
                    ))}
                  </div>
                ) : (
                  <RailLink key={item.href} link={item} arrow="always" className={ROW} onDone={close} />
                ),
              )}
              {signedIn ? (
                <>
                  <RailLink link={{ label: "Console", href: "/console" }} className={ROW} onDone={close} />
                  <SignOutButton
                    variant="ghost"
                    className={cn(ROW, "h-auto justify-start p-0 text-sm font-normal text-muted-foreground hover:bg-transparent hover:text-foreground")}
                  />
                </>
              ) : (
                <>
                  <RailLink link={{ label: "Sign in", href: "/auth/login" }} handler className={ROW} onDone={close} />
                  <RailLink link={{ label: "Sign up", href: "/auth/signup" }} handler className={ROW} onDone={close} />
                </>
              )}
              {DISCUSSIONS_URL && (
                <RailLink
                  link={{ label: "Discussions", href: DISCUSSIONS_URL, newTab: true }}
                  arrow="always"
                  className={ROW}
                  onDone={close}
                />
              )}
            </nav>
          </div>
        </div>
      )}
    </>
  );
}
