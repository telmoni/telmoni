import Link from "next/link";

import { HeaderShortcuts } from "@/components/header-shortcuts";
import { Button } from "@/components/ui/button";
import { MobileNav } from "@/components/mobile-nav";
import { SiteNavMenu } from "@/components/site-nav-menu";
import { TelmoniMark } from "@/components/telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";

const NAV_TYPE = "text-sm font-normal";

// The letter a button answers to (`header-shortcuts.tsx`), drawn as a keycap
// inside it. It takes its ink from the button's own text, so it reads on a
// filled button and on an outlined one alike.
function Keycap({ children }: { children: string }) {
  return (
    <kbd
      aria-hidden="true"
      className="ml-1 inline-flex h-5 min-w-5 items-center justify-center rounded-sm border border-current/30 bg-current/10 px-1 font-mono text-[11px] leading-none"
    >
      {children}
    </kbd>
  );
}

// ⚠ **`relative z-50` on the bar, and the panel is why.** A dropdown hanging
// out of it has to paint over the page, and the splash raises its own copy to
// `z-10`; with no z-index here the bar's subtree competes at the root and
// loses.
//
// The bar is a row of three cells, as a product site draws one: the brand,
// the navigation centred, and the two buttons, each cell ruled off from the
// next and the row ruled off from the page. A panel, when one returns,
// hangs from that rule.
export function SiteHeader({ signedIn }: { signedIn: boolean }) {
  return (
    <header className="relative z-50 shrink-0 border-b border-border bg-background">
      <div className="relative grid h-14 grid-cols-[auto_minmax(0,1fr)_auto] divide-x divide-border">
        <div className="flex items-center px-4">
          <Link
            href="/"
            className="flex items-center gap-2 text-[20px] leading-none font-bold tracking-tight hover:text-muted-foreground"
          >
            <TelmoniMark className="size-5" />
            {PRODUCT_NAME}
          </Link>
        </div>

        {/* The middle cell stays empty: the navigation sits over the row at
            the page's own centre, which the cells, unequal as the brand and the
            buttons are, would not give it. */}
        <div />
        <div className="absolute left-1/2 top-0 flex h-full -translate-x-1/2 items-center">
          <SiteNavMenu />
        </div>

        {/* A visitor gets the pair every row of buttons in the product uses:
            the other way out outlined, the thing you came to do filled, with
            its key. A member gets two outlined: the console is where they
            already are, not a thing to sell, and the key stays on it. Below
            `lg` neither shows; the drawer carries the same pair. */}
        <div className="flex items-center px-3">
          <div className="hidden items-center gap-2 lg:flex">
            {signedIn ? (
              <>
                <Button asChild variant="outline" size="sm" className={NAV_TYPE}>
                  {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                  <a href="/auth/logout">Sign out</a>
                </Button>
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
