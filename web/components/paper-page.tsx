import Link from "next/link";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { INSET } from "@/lib/inset";
import { ArrowRight, ArrowUpRight, Play } from "lucide-react";
import { cn } from "@/lib/utils";

// The pieces of a framed public page (`paper-shell.tsx`, `framed`): the
// splash's, and every page laid out as it is. A hero, then sections ruled
// apart in a two-column editorial layout, numbered where a page asks, each inset
// by the one measure; one outlined button a section ends on.

// The small line a section opens with: its number, as a document's
// sections are numbered, or what a page is, over a title in the console's
// own face.
export const EYEBROW = "text-xs uppercase tracking-label text-muted-foreground";

// A paragraph of a section.
export const BODY = "text-sm leading-[1.65] text-muted-foreground";

// The paragraph under a hero's heading, a size up.
export const LEAD = "max-w-[72ch] text-base leading-[1.65] text-muted-foreground lg:text-lg";

// The page's claim as its one `h1`, under the line that says what the page
// is, a badge before it naming a stage, then whatever the page opens with.
export function PaperHero({
  eyebrow,
  badge,
  title,
  children,
  className,
}: {
  eyebrow?: string;
  badge?: string;
  title: string;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <section
      data-component="hero"
      className={cn(
        "flex flex-col gap-4 pt-8 pb-8 lg:pt-20 lg:pb-10 lg:pr-16",
        INSET,
        className,
      )}
    >
      <div className="flex flex-col gap-5">
        {(badge !== undefined || eyebrow !== undefined) && (
          <div className="flex flex-wrap items-center gap-gutter">
            {badge !== undefined && (
              <Badge variant="outline" className="font-mono text-xs">
                {badge}
              </Badge>
            )}
            {eyebrow !== undefined && <p className={EYEBROW}>{eyebrow}</p>}
          </div>
        )}
        <h1 className="font-mono max-w-[44ch] text-[32px] font-medium leading-[1.15] tracking-tight text-foreground sm:text-[44px] lg:text-[50px]">
          {title}
        </h1>
      </div>
      {children}
    </section>
  );
}

// A two-column editorial block ruled off from the next: its title and number in
// the left column, its body and structured components in the right column.
export function PaperSection({
  id,
  number,
  title,
  children,
  className,
}: {
  id?: string;
  number?: string;
  title?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section
      id={id}
      data-component="block"
      data-columns="2"
      className={cn(
        "flex scroll-mt-24 flex-col py-12 lg:py-16",
        INSET,
        className,
      )}
    >
      {title !== undefined ? (
        <div className="grid grid-cols-1 gap-8 lg:grid-cols-12 lg:gap-12">
          <div className="flex flex-col gap-2 lg:col-span-3">
            {number !== undefined && (
              <span className="font-mono text-xs text-muted-foreground tracking-wider">
                [{number}]
              </span>
            )}
            <h2
              data-slot="label"
              className="text-base font-medium text-foreground"
            >
              {title}
            </h2>
          </div>
          <div data-slot="body" className="flex flex-col gap-8 lg:col-span-9">
            {children}
          </div>
        </div>
      ) : (
        <div className="flex flex-col gap-8">{children}</div>
      )}
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
// of it opens in a new tab, so the page stays, and says so with an arrow, as
// the rail's do, unless the caller marks it with an icon of its own (`arrow`).
export function PaperLink({
  href,
  children,
  arrow = true,
  ...rest
}: Omit<React.ComponentProps<"a">, "href" | "target" | "rel"> & { href: string; arrow?: boolean }) {
  if (href.startsWith("/")) {
    return (
      <Link href={href} {...rest}>
        {children}
      </Link>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer" {...rest}>
      {children}
      {arrow && <ArrowUpRight aria-hidden="true" className="ml-0.5 inline-block size-icon align-[-0.125em]" />}
    </a>
  );
}

// The one button a section ends on, outlined, its arrow pointing onward.
export function PaperGo({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <Button asChild variant="outline" size="sm" className="w-fit font-normal">
      <PaperLink href={href}>
        {children}
        <ArrowRight aria-hidden="true" />
      </PaperLink>
    </Button>
  );
}

// A 2-column feature list of key capabilities.
export function FeatureGrid({ children }: { children: React.ReactNode }) {
  return (
    <ul data-component="features" className="grid grid-cols-1 gap-x-8 gap-y-6 sm:grid-cols-2">
      {children}
    </ul>
  );
}

export function FeatureItem({
  title,
  description,
  href,
  linkText,
}: {
  title: string;
  description: React.ReactNode;
  href?: string;
  linkText?: string;
}) {
  return (
    <li className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5 text-base font-medium text-foreground">
        <span>{title}</span>
        {href && linkText && (
          <PaperLink
            href={href}
            arrow={false}
            className="inline-flex items-center gap-1 text-xs font-normal text-muted-foreground underline underline-offset-4 hover:text-foreground"
          >
            {linkText} <ArrowUpRight className="size-icon" aria-hidden="true" />
          </PaperLink>
        )}
      </div>
      <p className={BODY}>{description}</p>
    </li>
  );
}

export function StatsProof({
  stats,
  note,
  action,
}: {
  stats: readonly { label: string; value: string }[];
  note?: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-8">
      <dl className="grid grid-cols-2 gap-8 sm:grid-cols-3">
        {stats.map((s) => (
          <div key={s.label} className="flex flex-col gap-2">
            <dd className="font-mono text-4xl font-normal tracking-tight text-foreground sm:text-5xl">
              {s.value}
            </dd>
            <dt className="text-sm text-muted-foreground">
              {s.label}
            </dt>
          </div>
        ))}
      </dl>
      {note && <div className={BODY}>{note}</div>}
      {action && <div className="flex flex-wrap items-center gap-3">{action}</div>}
    </div>
  );
}

// 3-column surface card grid (Console, Terminal, SDK) inspired by editorial interfaces.
export function SurfaceGrid({ children }: { children: React.ReactNode }) {
  return (
    <div
      data-component="surface-grid"
      className="grid grid-cols-1 gap-gutter md:grid-cols-3"
    >
      {children}
    </div>
  );
}

// A surface card with video/media preview placeholder, title, description, and action button.
export function SurfaceCard({
  title,
  description,
  action,
  preview,
}: {
  title: string;
  description: string;
  action: React.ReactNode;
  preview: React.ReactNode;
}) {
  return (
    <div data-component="surface-card" className="flex h-full min-w-0 flex-col gap-4">
      <div tabIndex={0} role="region" aria-label={`${title} preview`} className="group relative aspect-[4/5] w-full overflow-y-auto overflow-x-hidden rounded-xl border border-border bg-foreground text-background select-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        {preview}
      </div>
      <div className="flex flex-1 flex-col gap-1.5">
        <h3 className="text-base font-medium text-foreground">
          {title}
        </h3>
        <p className={BODY}>{description}</p>
      </div>
      <div className="mt-auto pt-1">{action}</div>
    </div>
  );
}

function VideoPlayOverlay() {
  return (
    <>
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 flex items-center justify-center bg-foreground/20 transition-all duration-300 group-hover:bg-foreground/5"
      >
        <div className="flex h-12 w-12 items-center justify-center rounded-full border border-background/20 bg-background/10 text-background backdrop-blur-md shadow-2xl transition-transform duration-300 group-hover:scale-110">
          <Play className="size-icon fill-background ml-0.5" />
        </div>
      </div>
      <div
        aria-hidden="true"
        className="pointer-events-none flex items-center justify-between pt-3 font-mono text-[10px] tracking-wider text-background/60"
      >
        <span className="rounded-md bg-foreground/60 px-1.5 py-0.5 border border-background/10 text-background/90 backdrop-blur-sm">
          VIDEO PREVIEW
        </span>
        <span className="text-background/40">0:15</span>
      </div>
    </>
  );
}

// Video placeholder mockup for the Web Console surface.
export function ConsoleVideoPlaceholder() {
  return (
    <div className="relative min-h-full w-full p-4 flex flex-col justify-between gap-4 font-sans">
      <div className="flex items-center justify-between text-[11px] font-mono text-background/60 border-b border-background/10 pb-2.5">
        <div className="flex items-center gap-2">
          <span className="size-2 rounded-full bg-brand-positive animate-pulse" />
          <span className="text-background/90 font-medium">telmoni console</span>
        </div>
        <span className="text-background/40">prj_prod</span>
      </div>

      <div className="flex flex-col gap-2.5 my-auto">
        <div className="rounded-md border border-background/10 bg-background/5 p-3 backdrop-blur-sm">
          <div className="flex items-center justify-between text-xs">
            <span className="font-mono text-background/90 font-medium">agent-coder</span>
            <span className="rounded-md bg-brand-positive/20 px-1.5 py-0.5 font-mono text-[10px] text-brand-positive border border-brand-positive/30">
              Active
            </span>
          </div>
          <div className="mt-2 grid grid-cols-3 gap-2 border-t border-background/10 pt-2 text-[10px] font-mono text-background/60">
            <div>
              <span className="text-background/40 block">STEPS</span>
              <span className="text-background/80">14 / 50</span>
            </div>
            <div>
              <span className="text-background/40 block">COST</span>
              <span className="text-background/80">$0.042</span>
            </div>
            <div>
              <span className="text-background/40 block">GUARD</span>
              <span className="text-brand-positive">OK</span>
            </div>
          </div>
        </div>

        <div className="rounded-md border border-background/10 bg-background/5 p-2.5">
          <div className="flex items-center justify-between text-[11px] font-mono text-background/60">
            <span>Spend ceiling</span>
            <span className="text-background/80">$1.00 / session</span>
          </div>
          <div className="mt-1.5 h-1.5 w-full rounded-full bg-background/10 overflow-hidden">
            <div className="h-full w-1/4 rounded-full bg-brand-positive" />
          </div>
        </div>
      </div>

      <VideoPlayOverlay />
    </div>
  );
}

// Video placeholder mockup for the Terminal / CLI surface.
export function TerminalVideoPlaceholder() {
  return (
    <div className="relative min-h-full w-full p-4 flex flex-col justify-between gap-4 font-mono text-xs">
      <div className="flex items-center justify-between text-[11px] text-background/60 border-b border-background/10 pb-2.5">
        <div className="flex items-center gap-1.5">
          <span className="size-2 rounded-full bg-background/20" />
          <span className="size-2 rounded-full bg-background/20" />
          <span className="size-2 rounded-full bg-background/20" />
        </div>
        <span className="text-background/40">telmoni — status</span>
      </div>

      <div className="my-auto flex flex-col gap-1.5 text-[11px] leading-relaxed text-background/80">
        <p className="text-background">
          <span className="text-brand-positive">$</span> telmoni status
        </p>
        <div className="mt-1 flex flex-col gap-1 border-l-2 border-background/20 pl-2">
          {/* `telmoni status`'s own lines, as the CLI prints them. */}
          <p className="text-background/80">
            <span className="text-background/40">Signed in as</span> ada@example.com{" "}
            <span className="text-background/60">(Ada)</span>
          </p>
          <p className="text-background/80">
            <span className="text-background/40">Active organization:</span> Acme{" "}
            <span className="text-background/60">(3f2b9c1e-7d4a-4c1b-9e2f-5a6b7c8d9e0f, owner)</span>
          </p>
          <p className="text-background/80">
            <span className="text-background/40">Organizations:</span> 2
          </p>
          <p className="text-background/80">
            <span className="text-background/40">Endpoint:</span> https://api.telmoni.com
          </p>
          <p className="text-background/80">
            <span className="text-background/40">Token:</span> expires in 54 minutes
          </p>
        </div>
        <p className="text-background/40 text-[10px] mt-2">
          $ telmoni org list{" "}
          <span className="inline-block h-3 w-1.5 bg-background/80 animate-pulse align-middle ml-1" />
        </p>
      </div>

      <VideoPlayOverlay />
    </div>
  );
}

// Video placeholder mockup for the SDK surface.
export function SdkVideoPlaceholder() {
  return (
    <div className="relative min-h-full w-full p-4 flex flex-col justify-between gap-4 font-mono text-xs">
      <div className="flex items-center justify-between text-[11px] text-background/60 border-b border-background/10 pb-2.5">
        <div className="flex items-center gap-1.5">
          <span className="size-2 rounded-full bg-background/20" />
          <span className="size-2 rounded-full bg-background/20" />
          <span className="size-2 rounded-full bg-background/20" />
        </div>
        <span className="text-background/40">agent.ts</span>
      </div>

      <div className="my-auto flex flex-col gap-1 text-[11px] leading-relaxed">
        <p>
          <span className="text-brand-unit">import</span>{" "}
          <span className="text-background">{"{ Telmoni }"}</span>{" "}
          <span className="text-brand-unit">from</span>{" "}
          <span className="text-brand-positive">&quot;telmoni&quot;</span>;
        </p>
        <div className="my-1" />
        <p>
          <span className="text-brand-unit">const</span>{" "}
          <span className="text-background">telmoni</span> ={" "}
          <span className="text-background">Telmoni.fromEnv()</span>;
        </p>
        <div className="my-1" />
        <p>
          <span className="text-brand-unit">await</span>{" "}
          <span className="text-background">telmoni.run</span>(
        </p>
        <p className="pl-3">
          <span className="text-brand-positive">&quot;support-bot&quot;</span>,
        </p>
        <p className="pl-3">
          {"{ "}
          <span className="text-background/60">maxSteps:</span>{" "}
          <span className="text-brand-unit">20</span>,{" "}
          <span className="text-background/60">maxSpend:</span>{" "}
          <span className="text-brand-unit">0.50</span>
          {" },"}
        </p>
        <p className="pl-3">
          <span className="text-brand-unit">async</span> (
          <span className="text-background">run</span>) =&gt; {"{ /* ... */ }"}
        </p>
        <p>{");"}</p>
      </div>

      <VideoPlayOverlay />
    </div>
  );
}
