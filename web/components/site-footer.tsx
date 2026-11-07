import Link from "next/link";

import { CornersToggle } from "@/components/corners-toggle";
import { ThemeToggle } from "@/components/theme-toggle";

import { footerColumns, type FooterLink } from "@/lib/footer";
import { INSET } from "@/lib/inset";
import { branding } from "@/lib/server/branding";
import { cn } from "@/lib/utils";

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

// The columns a product site draws, four or five where a deployment has legal
// documents, and the copyright at the foot: what each holds is
// `lib/footer.ts`'s to decide.
// `inset` is a framed public page's column, where the columns start where
// its sections do.
export function SiteFooter({ inset = false }: { inset?: boolean }) {
  const deployment = { legalUrl: branding.LEGAL_URL, supportEmail: branding.SUPPORT_EMAIL };
  const drawn = footerColumns(deployment);
  const pad = inset ? INSET : "px-gutter";
  return (
    <footer className="border-t bg-muted/30">
      <div className={cn("grid w-full grid-cols-2 gap-10 py-12 sm:grid-cols-4", drawn.length > 4 && "lg:grid-cols-5", pad)}>
        {drawn.map((column) => (
          <nav key={column.title} aria-label={column.title}>
            <h2 className="text-xs font-medium tracking-label text-foreground uppercase">
              {column.title}
            </h2>
            <ul className="mt-gutter flex flex-col gap-2">
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
      <div
        className={cn(
          "flex w-full flex-col gap-4 border-t py-page text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between",
          pad,
        )}
      >
        <span>© {new Date().getFullYear()} {branding.COMPANY_NAME}. All rights reserved.</span>
        <div className="flex items-center gap-gutter">
          <ThemeToggle />
          <CornersToggle />
        </div>
      </div>
    </footer>
  );
}
