import Link from "next/link";

import { CornersToggle } from "@/components/corners-toggle";
import { ThemeToggle } from "@/components/theme-toggle";

import { footerColumns } from "@/lib/footer";
import { branding } from "@/lib/server/branding";

export function SiteFooter() {
  const drawn = footerColumns({
    legalUrl: branding.LEGAL_URL,
    supportEmail: branding.SUPPORT_EMAIL,
  });
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
                  {link.href.startsWith("/") ? (
                    <Link
                      href={link.href}
                      {...(link.newTab && { target: "_blank" })}
                      className="text-sm text-muted-foreground hover:text-foreground"
                    >
                      {link.label}
                    </Link>
                  ) : (
                    <a
                      href={link.href}
                      target="_blank"
                      rel="noreferrer"
                      className="text-sm text-muted-foreground hover:text-foreground"
                    >
                      {link.label}
                    </a>
                  )}
                </li>
              ))}
            </ul>
          </nav>
        ))}
      </div>
      <div className="flex w-full flex-col gap-4 border-t px-3.5 py-6 text-xs text-muted-foreground sm:flex-row sm:items-center sm:justify-between">
        <span>© {new Date().getFullYear()} {branding.COMPANY_NAME}. All rights reserved.</span>
        <div className="flex items-center gap-3">
          <ThemeToggle />
          <CornersToggle />
        </div>
      </div>
    </footer>
  );
}
