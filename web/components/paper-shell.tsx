import Link from "next/link";
import { ArrowUpRight } from "lucide-react";

import { GithubIcon } from "@/components/brand-icons";
import { CornersToggle } from "@/components/corners-toggle";
import { StatusLabel } from "@/components/status-label";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { SignOutButton } from "@/components/sign-out-button";
import { RailLink } from "@/components/rail-link";
import { RailShortcuts, type RailShortcut } from "@/components/rail-shortcuts";
import { navLinks } from "@/components/site-nav";
import { TelmoniLockup } from "@/components/telmoni-mark";
import { ThemeToggle } from "@/components/theme-toggle";
import { footerColumns } from "@/lib/footer";
import { INSET } from "@/lib/inset";
import { branding } from "@/lib/server/branding";
import { PRODUCT_NAME, REPO_URL } from "@/lib/site";
import { cn } from "@/lib/utils";

// The public pages' shell. A framed page has `rail` on by default: desktop
// (`lg+`) shows a left rail, the brand and primary links sticky at its top,
// the legal links and theme/corner toggles scrolling into view at its foot.
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
      className="group inline-flex items-center gap-1 text-sm text-muted-foreground transition-colors hover:text-foreground"
    >
      <StatusLabel label={statusLink.label} services={statusLink.services} />
      <ArrowUpRight className="size-icon opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 motion-reduce:transition-none" aria-hidden="true" />
    </a>
  ) : null;
  const language = (
    <label className="group flex items-center gap-gutter text-sm text-muted-foreground">
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
            <nav aria-label="Primary" className="flex flex-col gap-gutter text-sm">
              {primary.map((link) => (
                <RailLink key={link.href} link={link} />
              ))}
              {signedIn ? (
                <>
                <RailLink link={{ label: "Console", href: "/console" }} />
                <SignOutButton variant="ghost" className="h-auto justify-start p-0 text-sm font-normal text-muted-foreground hover:bg-transparent hover:text-foreground" />
                </>
              ) : (
                <>
                  <RailLink link={{ label: "Sign in", href: "/auth/login" }} handler />
                  <RailLink link={{ label: "Sign up", href: "/auth/signup" }} handler />
                </>
              )}
            </nav>
          </div>
          </div>

          <div data-rail-footer className="flex flex-col gap-gutter pt-gutter transition-[opacity,translate,visibility] duration-300 ease-out group-data-[footer-visible=false]/rail:invisible group-data-[footer-visible=false]/rail:translate-y-gutter group-data-[footer-visible=false]/rail:opacity-0 motion-reduce:transition-none">
            {metaLinks.length > 0 && (
              <nav aria-label="Secondary" className="flex flex-col gap-gutter text-sm">
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
          {/* Pinned on a phone, so the brand and the menu stay where the
              menu's sheet (`mobile-nav.tsx`, under the bar) expects them. */}
          <div className="sticky top-0 z-50 lg:hidden">
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
                className="pointer-events-auto flex size-6 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
              >
                <GithubIcon className="size-6" />
              </a>
            </div>
          )}

          <div className="flex flex-1 flex-col">{children}</div>

          <footer className={cn("flex flex-col gap-gutter py-8 lg:hidden", INSET)}>
            {metaLinks.length > 0 && (
              <nav aria-label="Footer" className="flex flex-col gap-gutter text-sm">
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
