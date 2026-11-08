/* eslint-disable @next/next/no-html-link-for-pages */
import Link from "next/link";
import { ArrowUpRight } from "lucide-react";

import { CornersToggle } from "@/components/corners-toggle";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { SignOutButton } from "@/components/sign-out-button";
import { RailShortcuts, type RailShortcut } from "@/components/rail-shortcuts";
import { navLinks, type NavLink } from "@/components/site-nav";
import { TelmoniLockup } from "@/components/telmoni-mark";
import { ThemeToggle } from "@/components/theme-toggle";
import { footerColumns, type FooterLink } from "@/lib/footer";
import { INSET } from "@/lib/inset";
import { branding } from "@/lib/server/branding";
import { PRODUCT_NAME, REPO_URL } from "@/lib/site";
import { cn } from "@/lib/utils";


function RailLink({
  link,
  className,
}: {
  link: NavLink | FooterLink;
  className?: string;
}) {
  const content = <>{link.label}{(link.newTab || !link.href.startsWith("/")) && <ArrowUpRight aria-hidden="true" className="ml-1 inline-block size-4 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 motion-reduce:transition-none" />}</>;
  if (link.href.startsWith("/")) {
    return (
      <Link
        href={link.href}
        {...(link.newTab && { target: "_blank", rel: "noreferrer" })}
        className={cn("group text-muted-foreground hover:text-foreground transition-colors", className)}
      >
        {content}
      </Link>
    );
  }
  return (
    <a
      href={link.href}
      target="_blank"
      rel="noreferrer"
      className={cn("group text-muted-foreground hover:text-foreground transition-colors", className)}
    >
      {content}
    </a>
  );
}

// The public pages' shell. On framed pages with `rail` on (the default for
// splash and plans), desktop (`lg+`) shows a left rail where the brand and
// primary links stay sticky at the top. Legal links and theme/corner toggles
// scroll into view at the bottom of the rail. Public surfaces (/,
// /plans, /docs) frame with `rail={true}`.
export function PaperShell({
  children,
  signedIn,
  className,
  framed = false,
  rail = framed,
}: {
  children: React.ReactNode;
  signedIn: boolean;
  className?: string;
  framed?: boolean;
  rail?: boolean;
}) {
  if (!framed || !rail) {
    return (
      <div
        className={cn(
          "theme-paper relative min-h-screen bg-background",
          className,
        )}
      >
        <div
          className={cn(
            "flex min-h-screen flex-col",
            framed && "mx-auto w-full max-w-[1320px]",
          )}
        >
          <SiteHeader signedIn={signedIn} inset={framed} />
          <div className="flex-1 flex flex-col">{children}</div>
          <SiteFooter inset={framed} />
        </div>
      </div>
    );
  }

  const primary = navLinks();
  const primaryHrefs = new Set(primary.map((l) => l.href));

  // Keep the existing digit shortcuts independent of their visual labels.
  const shortcuts: RailShortcut[] = primary.map((item, i) => ({
    key: String(i + 1),
    href: item.href,
    newTab: item.newTab,
  }));
  if (signedIn) {
    shortcuts.push({
      key: String(primary.length + 1),
      href: "/console",
    });
  } else {
    shortcuts.push({
      key: String(primary.length + 1),
      href: "/auth/login",
    });
    shortcuts.push({
      key: String(primary.length + 2),
      href: "/auth/signup",
    });
  }
  const deployment = { legalUrl: branding.LEGAL_URL, supportEmail: branding.SUPPORT_EMAIL };
  const footerLinks = footerColumns(deployment).flatMap((col) => col.links);
  const metaLinks = ["Security", "Terms", "Privacy", "Changelog"].flatMap((label, i) => {
    const link = footerLinks.find((item) => item.label === label && !primaryHrefs.has(item.href));
    return link ? [{ ...link, key: String(i + 6) }] : [];
  });
  shortcuts.push(...metaLinks.map(({ key, href, newTab }) => ({ key, href, newTab })));
  const statusLink = footerLinks.find((link) => link.label === "Status");
  const status = statusLink ? (
    <a
      href={statusLink.href}
      target="_blank"
      rel="noreferrer"
      className="group inline-flex items-center gap-1 font-mono text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      {/* No health is claimed: nothing measures it yet. */}
      {statusLink.label}
      <ArrowUpRight className="size-4 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 motion-reduce:transition-none" aria-hidden="true" />
    </a>
  ) : null;
  const language = (
    <label className="group flex items-center gap-gutter font-mono text-sm text-muted-foreground">
      <select aria-label="Language" defaultValue="en" className="min-w-0 appearance-none bg-transparent text-foreground" title="English is currently the only available language">
        <option value="en">English</option>
      </select>
    </label>
  );

  return (
    <div className={cn("theme-paper relative min-h-screen bg-background", className)}>
      <div className="mx-auto flex min-h-screen w-full max-w-[1600px] flex-col lg:flex-row">
        <RailShortcuts shortcuts={shortcuts} />
        <aside
          data-public-rail
          aria-label="Site navigation"
          className="group/rail hidden w-52 shrink-0 flex-col justify-between pb-8 pl-8 pr-4 lg:flex"
        >
          <div className="sticky top-0 z-20 shrink-0 bg-background py-8">
            <Link
              href="/"
              aria-label={PRODUCT_NAME}
              className="flex h-6 items-center transition-opacity hover:opacity-75"
            >
              <TelmoniLockup className="h-6 w-auto" />
            </Link>
          </div>
          <div className="min-h-0 flex-1">
          <div data-rail-primary className="sticky top-22 z-10 flex flex-col bg-background pb-4 transition-[opacity,translate,visibility] duration-300 ease-out group-data-[footer-visible=true]/rail:invisible group-data-[footer-visible=true]/rail:-translate-y-gutter group-data-[footer-visible=true]/rail:opacity-0 motion-reduce:transition-none">
            <nav aria-label="Primary" className="flex flex-col gap-gutter font-mono text-sm">
              {primary.map((link) => (
                <RailLink key={link.href} link={link} />
              ))}
              {signedIn ? (
                <>
                <Link
                  href="/console"
                  className="text-muted-foreground transition-colors hover:text-foreground"
                >
                  Console
                </Link>
                <SignOutButton variant="ghost" className="h-auto justify-start p-0 font-mono text-sm font-normal text-muted-foreground hover:bg-transparent hover:text-foreground" />
                </>
              ) : (
                <>
                  <a
                    href="/auth/login"
                    className="text-muted-foreground transition-colors hover:text-foreground"
                  >
                    Sign in
                  </a>
                  <a
                    href="/auth/signup"
                    className="text-muted-foreground transition-colors hover:text-foreground"
                  >
                    Sign up
                  </a>
                </>
              )}
            </nav>
          </div>
          </div>

          <div data-rail-footer className="flex flex-col gap-gutter pt-gutter transition-[opacity,translate,visibility] duration-300 ease-out group-data-[footer-visible=false]/rail:invisible group-data-[footer-visible=false]/rail:translate-y-gutter group-data-[footer-visible=false]/rail:opacity-0 motion-reduce:transition-none">
            {metaLinks.length > 0 && (
              <nav aria-label="Secondary" className="flex flex-col gap-gutter font-mono text-sm">
                {metaLinks.map((link) => (
                  <RailLink key={link.href} link={link} />
                ))}
              </nav>
            )}
            {language}
            {status}
            <div className="flex flex-col gap-4">
              <div className="flex flex-col items-start gap-gutter">
                <ThemeToggle />
                <CornersToggle />
              </div>
              <p className="text-xs text-muted-foreground">
                © {new Date().getFullYear()} {branding.COMPANY_NAME}
              </p>
            </div>
          </div>
        </aside>

        <div className="relative flex min-w-0 flex-1 flex-col">
          <div className="lg:hidden">
            <SiteHeader signedIn={signedIn} inset />
          </div>

          {/* Desktop GitHub link: clean minimal icon at top right, aligned with Telmoni lockup */}
          {REPO_URL && (
            <div className="pointer-events-none absolute top-8 right-8 z-20 hidden lg:flex">
              <a
                href={REPO_URL}
                target="_blank"
                rel="noreferrer"
                aria-label="GitHub repository"
                className="pointer-events-auto flex h-6 w-6 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
              >
                <svg className="size-5 fill-current" viewBox="0 0 24 24" aria-hidden="true">
                  <path
                    fillRule="evenodd"
                    clipRule="evenodd"
                    d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.53 1.032 1.53 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"
                  />
                </svg>
              </a>
            </div>
          )}

          <div className="flex flex-1 flex-col">{children}</div>

          <footer className={cn("flex flex-col gap-gutter py-8 lg:hidden", INSET)}>
            {metaLinks.length > 0 && (
              <nav aria-label="Footer" className="flex flex-col gap-gutter font-mono text-sm">
                {metaLinks.map((link) => (
                  <RailLink key={link.href} link={link} />
                ))}
              </nav>
            )}
            {language}
            {status}
            <div className="flex flex-col gap-4">
              <div className="flex flex-col items-start gap-gutter">
                <ThemeToggle />
                <CornersToggle />
              </div>
              <p className="text-xs text-muted-foreground">
                © {new Date().getFullYear()} {branding.COMPANY_NAME}
              </p>
            </div>
          </footer>
        </div>
      </div>
    </div>
  );
}
