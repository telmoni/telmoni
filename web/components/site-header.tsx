import Link from "next/link";

import { HeaderShortcuts } from "@/components/header-shortcuts";
import { Button } from "@/components/ui/button";
import { MobileNav } from "@/components/mobile-nav";
import { SignOutButton } from "@/components/sign-out-button";
import { SiteNavMenu } from "@/components/site-nav-menu";
import { TelmoniLockup } from "@/components/telmoni-mark";
import { INSET } from "@/lib/inset";
import { PRODUCT_NAME } from "@/lib/site";
import { cn } from "@/lib/utils";

const NAV_TYPE = "text-sm font-normal";

// The letter a button answers to (`header-shortcuts.tsx`), drawn as a keycap
// inside it. It takes its ink from the button's own text, so it reads on a
// filled button and on an outlined one alike, and hides while letter-key
// shortcuts are off (`data-letter-key`, `globals.css`).
function Keycap({ children }: { children: string }) {
  return (
    <kbd
      data-letter-key=""
      aria-hidden="true"
      className="ml-1 inline-flex h-5 min-w-5 items-center justify-center rounded-sm border border-current/30 bg-current/10 px-1 text-[11px] leading-none"
    >
      {children}
    </kbd>
  );
}

// ⚠ **`relative z-50` on the bar, and the panel is why.** A dropdown hanging
// out of it has to paint over the page; with no z-index here the bar's
// subtree competes at the root and loses.
//
// The bar is one row: the brand at the left and, at the right, the links
// and then the two buttons, the row ruled off from the page. Edge to edge,
// the brand stands on the console header's pixels — the
// same 60px row, the mark at the same x (`px-3.5` on the cell and `pl-1` on
// the link, as there), the same 17px name — so signing in moves nothing but
// the page under it. `inset` is a framed public page's column: the row
// taller, and its cells inset as the sections are.
export function SiteHeader({ signedIn, inset = false }: { signedIn: boolean; inset?: boolean }) {
  return (
    <header
      className={cn(
        "relative z-50 h-15 shrink-0 border-b border-border bg-background",
        inset && "lg:h-24",
      )}
    >
      <div className="flex h-full items-center justify-between">
        <div className={cn("flex items-center", inset ? INSET : "px-3.5")}>
          <Link
            href="/"
            aria-label={PRODUCT_NAME}
            className={cn("flex min-w-0 items-center hover:opacity-70", !inset && "pl-1")}
          >
            <TelmoniLockup className="shrink-0" />
          </Link>
        </div>

        {/* A visitor gets the pair every row of buttons in the product uses:
            the other way out outlined, the thing you came to do filled, with
            its key. A member gets two outlined: the console is where they
            already are, not a thing to sell, and the key stays on it. Below
            `lg` neither shows; the drawer carries the same pair. */}
        <div className={cn("flex items-center gap-3", inset ? INSET : "px-3")}>
          <SiteNavMenu />
          <div className="hidden items-center gap-2 lg:flex">
            {signedIn ? (
              <>
                <SignOutButton className={NAV_TYPE} />
                <Button asChild variant="outline" size="sm" className={NAV_TYPE}>
                  <Link href="/console">
                    Console
                    <Keycap>C</Keycap>
                  </Link>
                </Button>
              </>
            ) : (
              <>
                <Button asChild variant="outline" size="sm" className={NAV_TYPE}>
                  {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                  <a href="/auth/login">Sign in</a>
                </Button>
                <Button asChild size="sm" className={NAV_TYPE}>
                  {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                  <a href="/auth/signup">
                    Sign up
                    <Keycap>S</Keycap>
                  </a>
                </Button>
              </>
            )}
          </div>

          <div className="lg:hidden">
            <MobileNav signedIn={signedIn} />
          </div>
        </div>
      </div>
      <HeaderShortcuts signedIn={signedIn} />
    </header>
  );
}
