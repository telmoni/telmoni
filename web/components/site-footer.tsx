import Link from "next/link";

import { CornersToggle } from "@/components/corners-toggle";
import { ThemeToggle } from "@/components/theme-toggle";

import { footerColumns, footerLegal, type FooterLink } from "@/lib/footer";
import { branding } from "@/lib/server/branding";

// A link into the console is routed; one out of it opens in a new tab.
function FooterAnchor({ link, className }: { link: FooterLink; className: string }) {
  return link.href.startsWith("/") ? (
    <Link href={link.href} {...(link.newTab && { target: "_blank" })} className={className}>
      {link.label}
    </Link>
  ) : (
    <a href={link.href} target="_blank" rel="noreferrer" className={className}>
      {link.label}
    </a>
  );
}

// Four columns, as a product site draws them, and the legal line at the foot
// beside the copyright: what each holds is `lib/footer.ts`'s to decide.
export function SiteFooter() {
  const deployment = { legalUrl: branding.LEGAL_URL, supportEmail: branding.SUPPORT_EMAIL };
  const drawn = footerColumns(deployment);
  const legal = footerLegal(deployment);
  return (
    <footer className="border-t bg-muted/30">
      <div className="grid w-full grid-cols-2 gap-10 px-3.5 py-12 sm:grid-cols-4">
        {drawn.map((column) => (
          <nav key={column.title} aria-label={column.title}>
            <h2 className="text-xs font-medium tracking-label text-foreground uppercase">
              {column.title}
            </h2>
            <ul className="mt-3 flex flex-col gap-2">
              {column.links.map((link) => (
                <li key={link.href}>
                  <FooterAnchor
                    link={link}
                    className="text-sm text-muted-foreground hover:text-foreground"
                  />
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
      <div className="flex w-full flex-col gap-4 border-t px-3.5 py-6 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <span>© {new Date().getFullYear()} {branding.COMPANY_NAME}. All rights reserved.</span>
          {legal.length > 0 && (
            <nav aria-label="Legal" className="flex flex-wrap items-center gap-x-4 gap-y-2">
              {legal.map((link) => (
                <FooterAnchor key={link.href} link={link} className="hover:text-foreground" />
              ))}
            </nav>
          )}
        </div>
        <div className="flex items-center gap-3">
          <ThemeToggle />
          <CornersToggle />
        </div>
      </div>
    </footer>
  );
}
