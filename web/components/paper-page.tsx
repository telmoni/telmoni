import Link from "next/link";

import { Button } from "@/components/ui/button";
import { INSET } from "@/lib/inset";
import { cn } from "@/lib/utils";

// The pieces of a framed public page (`paper-shell.tsx`, `framed`): the
// splash's, and every page laid out as it is. A hero, then sections ruled
// apart and numbered, each inset by the one measure; a list marked as the
// monogram is drawn; one outlined button a section ends on.

// The small mono line a section opens with: its number, as a document's
// sections are numbered, or what a page is, over a title in the console's
// own face.
export const EYEBROW = "font-mono text-[11px] uppercase tracking-label text-muted-foreground";

// A paragraph of a section.
export const BODY = "text-[15px] leading-[1.65] text-muted-foreground";

// The paragraph under a hero's heading, a size up.
export const LEAD = "max-w-[58ch] text-base leading-[1.65] text-muted-foreground lg:text-lg";

// The page's claim as its one `h1`, under the line that says what the page
// is, then whatever the page opens with: its lead, a way in.
export function PaperHero({
  eyebrow,
  title,
  children,
}: {
  eyebrow?: string;
  title: string;
  children?: React.ReactNode;
}) {
  return (
    <section
      className={cn(
        "flex flex-col gap-8 border-b border-border pt-16 pb-14 lg:pt-32 lg:pb-24",
        INSET,
      )}
    >
      <div className="flex flex-col gap-5">
        {eyebrow !== undefined && <p className={EYEBROW}>{eyebrow}</p>}
        <h1 className="max-w-[30ch] text-[32px] font-semibold leading-[1.1] tracking-[-0.02em] text-balance text-foreground sm:text-[44px] lg:text-[52px]">
          {title}
        </h1>
      </div>
      {children}
    </section>
  );
}

// A section of the column, ruled off from the next: its number and title,
// then what it holds. Without a title it is a band at the same inset and
// rhythm, as a page's cards are. The last before the footer leaves the rule
// to the footer's own.
export function PaperSection({
  id,
  number,
  title,
  children,
}: {
  id?: string;
  number?: string;
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <section
      id={id}
      className={cn(
        "flex scroll-mt-24 flex-col gap-6 border-b border-border py-12 last:border-b-0 lg:py-16",
        INSET,
      )}
    >
      {title !== undefined && (
        <div className="flex flex-col gap-2">
          {number !== undefined && <p className={EYEBROW}>{number}</p>}
          <h2 className="text-xl font-semibold tracking-tight text-foreground">{title}</h2>
        </div>
      )}
      {children}
    </section>
  );
}

// One line of a list, marked as the monogram's `m` is drawn — a short bar in
// its mid grey — then a term in the page's ink and its sentence.
export function PaperPoint({ title, children }: { title?: string; children: React.ReactNode }) {
  return (
    <li className={cn("flex gap-4", BODY)}>
      <span aria-hidden="true" className="mt-[0.75em] h-0.5 w-3 shrink-0 bg-mark-mid" />
      <span>
        {title !== undefined && <strong className="font-semibold text-foreground">{title}</strong>}
        {title !== undefined && " "}
        {children}
      </span>
    </li>
  );
}

// A link into the console — the book among its pages — is routed; one out
// of it opens in a new tab, so the page stays.
export function PaperLink({
  href,
  className,
  children,
}: {
  href: string;
  className?: string;
  children: React.ReactNode;
}) {
  if (href.startsWith("/")) {
    return (
      <Link href={href} className={className}>
        {children}
      </Link>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer" className={className}>
      {children}
    </a>
  );
}

// The one button a section ends on, outlined, its arrow pointing onward.
export function PaperGo({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <Button asChild variant="outline" size="sm" className="w-fit font-normal">
      <PaperLink href={href}>{children} →</PaperLink>
    </Button>
  );
}
