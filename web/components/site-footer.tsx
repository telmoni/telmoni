import { CornersToggle } from "@/components/corners-toggle";
import { RailLink } from "@/components/rail-link";
import { ThemeToggle } from "@/components/theme-toggle";

import { footerColumns } from "@/lib/footer";
import { INSET } from "@/lib/inset";
import { branding } from "@/lib/server/branding";
import { cn } from "@/lib/utils";

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
                  {/* The rail's link: a new tab's arrow on hover, as there. */}
                  <RailLink link={link} className="text-sm" />
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
