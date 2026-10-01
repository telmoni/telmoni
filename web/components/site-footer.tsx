import Link from "next/link";

import { CornersToggle } from "@/components/corners-toggle";
import { ThemeToggle } from "@/components/theme-toggle";

import { branding } from "@/lib/server/branding";
import {
  DISCUSSIONS_URL,
  DOCS_URL,
  REPO_URL,
  STATUS_URL,
  TWITTER_URL,
} from "@/lib/site";

type FooterLink = { label: string; href: string; newTab?: boolean };
type FooterColumn = { title: string; links: FooterLink[] };

// A column with nothing in it is not drawn: the legal and contact columns
// exist only where the deployment configured a legal base or an address, so
// a self-hosted console offers no document it does not have.
function columns(): FooterColumn[] {
  const legal = branding.LEGAL_URL;
  const support = branding.SUPPORT_EMAIL;
  return [
    {
      title: "Resources",
      links: [
        { label: "Docs", href: DOCS_URL, newTab: true },
        ...(STATUS_URL ? [{ label: "Status", href: STATUS_URL, newTab: true }] : []),
      ],
    },
    {
      title: "Legal",
      links: legal
        ? [
            { label: "Privacy Policy", href: `${legal}/privacy-policy`, newTab: true },
            { label: "Terms of Service", href: `${legal}/terms-of-service`, newTab: true },
          ]
        : [],
    },
    {
      title: "Contact",
      links: [
        ...(support ? [{ label: "Email", href: `mailto:${support}` }] : []),
        ...(REPO_URL ? [{ label: "GitHub", href: REPO_URL }] : []),
        ...(TWITTER_URL ? [{ label: "Twitter", href: TWITTER_URL }] : []),
        ...(DISCUSSIONS_URL ? [{ label: "Discussions", href: DISCUSSIONS_URL }] : []),
      ],
    },
  ].filter((column) => column.links.length > 0);
}

export function SiteFooter() {
  const drawn = columns();
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
