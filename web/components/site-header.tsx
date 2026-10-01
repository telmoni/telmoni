import { MessagesSquare } from "lucide-react";
import Link from "next/link";

import { GithubIcon } from "@/components/brand-icons";
import { Button } from "@/components/ui/button";
import { MobileNav } from "@/components/mobile-nav";
import { SiteNavMenu } from "@/components/site-nav-menu";
import { DISCUSSIONS_URL, PRODUCT_NAME, REPO_URL } from "@/lib/site";

const NAV_TYPE = "text-sm font-normal";

// ⚠ **`relative z-50` on the bar, and the panel is why.** A dropdown hanging
// out of it has to paint over the page, and the splash raises its own copy to
// `z-10`; with no z-index here the bar's subtree competes at the root and
// loses. No `border-b`: the bar carries no rule, so the panel draws its own
// top edge rather than leaning on one.
export function SiteHeader({ signedIn }: { signedIn: boolean }) {
  return (
    <header
      className="relative z-50 flex shrink-0 items-center justify-between bg-background px-3.5 py-3.5"
    >
      {/* `gap-8`, wider than the row's `gap-3`: the wordmark has no padding
          of its own, so at 12px it crowds the first nav link. */}
      <div className="flex items-center gap-8 self-stretch">
        <Link
          href="/"
          className="text-[20px] leading-none font-bold tracking-wider uppercase hover:text-muted-foreground"
        >
          {PRODUCT_NAME}
        </Link>
        <SiteNavMenu />
      </div>

      <div className="flex items-center gap-3">
        {/* The console header's icon buttons: 44 × 32, `gap-3` apart. */}
        <div className="hidden items-center gap-3 lg:flex">
          {REPO_URL && (
            <Button asChild variant="ghost" size={null} className="h-8 w-11">
              <a href={REPO_URL} target="_blank" rel="noreferrer" aria-label="GitHub">
                <GithubIcon />
              </a>
            </Button>
          )}
          {DISCUSSIONS_URL && (
            <Button asChild variant="ghost" size={null} className="h-8 w-11">
              <a
                href={DISCUSSIONS_URL}
                target="_blank"
                rel="noreferrer"
                aria-label="Discussions"
              >
                <MessagesSquare className="size-4" />
              </a>
            </Button>
          )}
          {(REPO_URL || DISCUSSIONS_URL) && (
            <span aria-hidden className="h-3 w-px bg-border" />
          )}
        </div>

        {/* The pair every row of buttons in the product uses: the other way
            out outlined, the thing you came to do filled — Sign in and Sign
            up, or Sign out and the console. Below `lg` neither shows; the
            drawer carries the same pair. */}
        <div className="hidden items-center gap-3 lg:flex">
          {signedIn ? (
            <>
              <Button asChild variant="outline" size="sm" className={NAV_TYPE}>
                {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
                <a href="/auth/logout">Sign out</a>
              </Button>
              <Button asChild size="sm" className={NAV_TYPE}>
                <Link href="/console">Console</Link>
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
                <a href="/auth/signup">Sign up</a>
              </Button>
            </>
          )}
        </div>

        <div className="lg:hidden">
          <MobileNav signedIn={signedIn} />
        </div>
      </div>
    </header>
  );
}
