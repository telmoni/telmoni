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

// ⚠ **`relative z-50` on the bar, and the panel is why.** A dropdown hanging
// out of it has to paint over the page; with no z-index here the bar's
// subtree competes at the root and loses.
//
// The bar is one row: the brand at the left and, at the right, the links
// and then the two buttons, the row ruled off from the page. Edge to edge,
// the brand stands on the console header's pixels — the
// same 60px row, the mark at the same x (`px-gutter` on the cell and `pl-1` on
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
        <div className={cn("flex items-center", inset ? INSET : "px-gutter")}>
          <Link
            href="/"
            aria-label={PRODUCT_NAME}
            className={cn("flex min-w-0 items-center hover:opacity-70", !inset && "pl-1")}
          >
            <TelmoniLockup className="shrink-0" />
          </Link>
        </div>

        {/* The mobile drawer carries the same account actions below `lg`. */}
        <div className={cn("flex items-center gap-gutter", inset ? INSET : "px-gutter")}>
          <SiteNavMenu />
          <div className="hidden items-center gap-gutter lg:flex">
            {signedIn ? (
              <>
                <SignOutButton className={NAV_TYPE} />
                <Button asChild variant="filled" size="sm" className={NAV_TYPE}>
                  <Link href="/console">
                    Console
                  </Link>
                </Button>
              </>
            ) : (
              <>
                <Button asChild variant="outline" size="sm" className={NAV_TYPE}>
                  {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                  <a href="/auth/login">Sign in</a>
                </Button>
                <Button asChild variant="filled" size="sm" className={NAV_TYPE}>
                  {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                  <a href="/auth/signup">
                    Sign up
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
