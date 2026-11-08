/* eslint-disable @next/next/no-html-link-for-pages */
"use client";

import type * as PageTree from "fumadocs-core/page-tree";
import { AnchorProvider, type TOCItemType, useActiveAnchor } from "fumadocs-core/toc";
import { useSearchContext } from "fumadocs-ui/contexts/search";
import { ArrowUpRight, ChevronDown, ChevronRight, ChevronUp } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import { CornersToggle } from "@/components/corners-toggle";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { BODY, LEAD, PaperHero } from "@/components/paper-page";
import { RailShortcuts, type RailShortcut } from "@/components/rail-shortcuts";
import { type NavLink } from "@/components/site-nav";
import { TelmoniLockup } from "@/components/telmoni-mark";
import { ThemeToggle } from "@/components/theme-toggle";
import { type FooterLink } from "@/lib/footer";
import { INSET } from "@/lib/inset";
import { PRODUCT_NAME } from "@/lib/site";
import { useIsMac } from "@/lib/use-platform";
import { cn } from "@/lib/utils";

interface DocsMenuSection {
  id: string;
  label: string;
  href: string;
  match?: (pathname: string) => boolean;
  external?: boolean;
}

const DOCS_SECTIONS = [
  {
    id: "docs",
    label: "DOCS",
    href: "/docs",
    match: (p: string) =>
      p === "/docs" ||
      p.startsWith("/docs/getting-started") ||
      p.startsWith("/docs/monitoring") ||
      p.startsWith("/docs/integrations") ||
      p.startsWith("/docs/workspace") ||
      p.startsWith("/docs/settings") ||
      p.startsWith("/docs/legal"),
  },
  {
    id: "cli",
    label: "CLI",
    href: "/docs/api/cli",
    match: (p: string) => p.startsWith("/docs/api/cli"),
  },
  {
    id: "sdk",
    label: "SDK",
    href: "/docs/api/sdks",
    match: (p: string) =>
      p.startsWith("/docs/api/sdks") ||
      p.startsWith("/docs/api/coding-agents") ||
      p.startsWith("/docs/api/frameworks"),
  },
  {
    id: "api",
    label: "API",
    href: "/docs/api/reference",
    match: (p: string) =>
      p.startsWith("/docs/api/reference") ||
      p.startsWith("/docs/api/ingest") ||
      p.startsWith("/docs/api/api-keys") ||
      p.startsWith("/docs/api/field-inventory") ||
      p.startsWith("/docs/errors"),
  },
  {
    id: "self-host",
    label: "SELF-HOST",
    href: "/docs/self-host/overview",
    match: (p: string) => p.startsWith("/docs/self-host") || p.startsWith("/docs/contributing"),
  },
] as const satisfies readonly DocsMenuSection[];

function SiteLinkItem({
  link,
  onClick,
}: {
  link: { label: string; href: string; newTab?: boolean };
  onClick?: () => void;
}) {
  const leaves = link.newTab || !link.href.startsWith("/");
  // The public rail's cue for a new tab: drawn on hover or keyboard focus.
  const content = (
    <>
      {link.label}
      {leaves && (
        <ArrowUpRight
          aria-hidden="true"
          className="ml-1 inline-block size-4 opacity-0 transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 motion-reduce:transition-none"
        />
      )}
    </>
  );
  const className =
    "group text-sm text-muted-foreground transition-colors hover:text-foreground py-0.5";
  if (leaves) {
    return (
      <a
        href={link.href}
        target="_blank"
        rel="noreferrer"
        onClick={onClick}
        className={className}
      >
        {content}
      </a>
    );
  }
  return (
    <Link href={link.href} onClick={onClick} className={className}>
      {content}
    </Link>
  );
}

function DocsTreeLink({
  item,
  pathname,
  onNavigate,
}: {
  item: PageTree.Item;
  pathname: string;
  onNavigate?: () => void;
}) {
  const active = pathname === item.url || pathname === `${item.url}/`;
  return (
    <Link
      href={item.url}
      onClick={onNavigate}
      aria-current={active ? "page" : undefined}
      className={cn(
        "group flex items-baseline gap-2 py-1 font-mono text-sm transition-colors",
        active ? "font-medium text-foreground" : "text-muted-foreground hover:text-foreground",
      )}
    >
      <span className="truncate">{item.name}</span>
    </Link>
  );
}

function DocsTreeNodes({
  nodes,
  pathname,
  onNavigate,
}: {
  nodes: PageTree.Node[];
  pathname: string;
  onNavigate?: () => void;
}) {
  return (
    <>
      {nodes.map((node, i) => {
        if (node.type === "page") {
          return (
            <DocsTreeLink
              key={node.url}
              item={node}
              pathname={pathname}
              onNavigate={onNavigate}
            />
          );
        }
        if (node.type === "separator") {
          return (
            <p
              key={`sep-${i}`}
              className="pt-6 pb-1.5 font-mono text-[11px] uppercase tracking-wider text-muted-foreground/70"
            >
              {node.name}
            </p>
          );
        }
        if (node.type === "folder") {
          return (
            <div key={node.$id ?? `folder-${i}`} className="flex flex-col">
              {node.name && (
                <p className="pt-6 pb-1.5 font-mono text-[11px] uppercase tracking-wider text-muted-foreground/70">
                  {node.name}
                </p>
              )}
              {node.index && (
                <DocsTreeLink
                  item={node.index}
                  pathname={pathname}
                  onNavigate={onNavigate}
                />
              )}
              <DocsTreeNodes
                nodes={node.children}
                pathname={pathname}
                onNavigate={onNavigate}
              />
            </div>
          );
        }
        return null;
      })}
    </>
  );
}

export function DocsShell({
  tree,
  signedIn,
  primaryLinks,
  metaLinks,
  companyName,
  children,
}: {
  tree: PageTree.Root;
  signedIn: boolean;
  primaryLinks: NavLink[];
  metaLinks: FooterLink[];
  companyName: string;
  children: React.ReactNode;
}) {
  const pathname = usePathname();
  const { setOpenSearch } = useSearchContext();
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const activeSection =
    DOCS_SECTIONS.find((s) => s.match?.(pathname)) ?? DOCS_SECTIONS[0];
  const consoleHref = signedIn ? "/console" : "/auth/login";
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  const railContentRef = useRef<HTMLDivElement>(null);
  const [railStickyTop, setRailStickyTop] = useState<number>(88);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileMenuOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const el = railContentRef.current;
    if (!el) return;
    const updateStickyTop = () => {
      // Keep a short tree below the brand; let a long tree scroll to its end.
      const minTop = window.innerHeight - el.offsetHeight - 32;
      setRailStickyTop(Math.min(88, minTop));
    };
    updateStickyTop();
    const ro = new ResizeObserver(updateStickyTop);
    ro.observe(el);
    window.addEventListener("resize", updateStickyTop);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", updateStickyTop);
    };
  }, [tree]);

  const shortcuts: RailShortcut[] = primaryLinks.map((item, i) => ({
    key: String(i + 1),
    href: item.href,
    newTab: item.newTab,
  }));
  if (signedIn) {
    shortcuts.push({
      key: String(primaryLinks.length + 1),
      href: "/console",
    });
  } else {
    shortcuts.push({
      key: String(primaryLinks.length + 1),
      href: "/auth/login",
    });
    shortcuts.push({
      key: String(primaryLinks.length + 2),
      href: "/auth/signup",
    });
  }

  return (
    <div className="theme-paper relative min-h-screen bg-background">
      <div className="mx-auto flex min-h-screen w-full max-w-[1600px] flex-col lg:flex-row">
        <RailShortcuts shortcuts={shortcuts} />

        {/* Mobile top bar, framed as the splash's header is, so the mark stays put */}
        <header
          className={cn(
            "flex h-15 items-center justify-between border-b border-border lg:hidden",
            INSET,
          )}
        >
          <div className="flex items-center gap-3">
            <Link
              href="/"
              aria-label={PRODUCT_NAME}
              className="flex h-6 items-center text-foreground transition-opacity hover:opacity-75"
            >
              <TelmoniLockup markOnly className="h-6 w-auto" />
            </Link>
            <button
              type="button"
              aria-expanded={mobileMenuOpen}
              onClick={() => setMobileMenuOpen((v) => !v)}
              className="inline-flex h-control shrink-0 cursor-pointer items-center whitespace-nowrap rounded-md bg-muted font-mono text-sm font-medium uppercase tracking-wider text-foreground transition-colors hover:bg-muted/80"
            >
              <span className="px-2">{activeSection.label}</span>
              <span className="flex h-full items-center border-l border-background px-1.5 text-muted-foreground">
                {mobileMenuOpen ? (
                  <ChevronUp className="size-3" aria-hidden="true" />
                ) : (
                  <ChevronDown className="size-3" aria-hidden="true" />
                )}
              </span>
            </button>
          </div>
          <button
            type="button"
            onClick={() => setOpenSearch(true)}
            className="cursor-pointer rounded-md bg-muted/80 px-3 py-1.5 font-mono text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          >
            Search
          </button>
        </header>

        {/* Mobile navigation drawer */}
        {mobileMenuOpen && (
          <div className={cn("flex flex-col gap-6 pb-8 lg:hidden", INSET)}>
            <nav aria-label="Docs navigation" className="flex flex-col">
              <DocsTreeNodes
                nodes={tree.children}
                pathname={pathname}
                onNavigate={() => setMobileMenuOpen(false)}
              />
            </nav>
            <div className="flex flex-col gap-2 pt-4 font-mono text-sm">
              <p className="pb-1 font-mono text-[11px] uppercase tracking-wider text-muted-foreground/70">
                SITE
              </p>
              {primaryLinks.map((link) => (
                <SiteLinkItem
                  key={link.href}
                  link={link}
                  onClick={() => setMobileMenuOpen(false)}
                />
              ))}
              {signedIn ? (
                <Link
                  href="/console"
                  onClick={() => setMobileMenuOpen(false)}
                  className="py-0.5 text-muted-foreground transition-colors hover:text-foreground"
                >
                  Console
                </Link>
              ) : (
                <>
                  <a
                    href="/auth/login"
                    className="py-0.5 text-muted-foreground transition-colors hover:text-foreground"
                  >
                    Sign in
                  </a>
                  <a
                    href="/auth/signup"
                    className="py-0.5 text-muted-foreground transition-colors hover:text-foreground"
                  >
                    Sign up
                  </a>
                </>
              )}
            </div>
          </div>
        )}

        {/* Desktop Left Rail: Brand Mark + DOCS switcher + [ ] / [-] Docs Tree */}
        <aside
          aria-label="Documentation navigation"
          className="hidden w-52 shrink-0 flex-col pb-8 pl-8 pr-4 lg:flex"
        >
          <div className="sticky top-0 z-20 flex items-center gap-2.5 bg-background pt-8 pb-4">
            <Link
              href="/"
              aria-label={PRODUCT_NAME}
              className="flex h-6 items-center text-foreground transition-opacity hover:opacity-75"
            >
              <TelmoniLockup markOnly className="h-6 w-auto" />
            </Link>
            <DropdownMenu open={switcherOpen} onOpenChange={setSwitcherOpen}>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="inline-flex h-control shrink-0 cursor-pointer items-center whitespace-nowrap rounded-md bg-muted font-mono text-sm font-medium uppercase tracking-wider text-foreground transition-colors hover:bg-muted/80"
                >
                  <span className="px-2">{activeSection.label}</span>
                  <span className="flex h-full items-center border-l border-background px-1.5 text-muted-foreground">
                    {switcherOpen ? (
                      <ChevronUp className="size-3" aria-hidden="true" />
                    ) : (
                      <ChevronDown className="size-3" aria-hidden="true" />
                    )}
                  </span>
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="min-w-40">
                {DOCS_SECTIONS.map((sec) => (
                  <DropdownMenuItem key={sec.id} asChild className="justify-between gap-6 uppercase tracking-wider">
                    <Link href={sec.href}>
                      <span>{sec.label}</span>
                      {sec.id === activeSection.id && <ChevronRight aria-hidden="true" />}
                    </Link>
                  </DropdownMenuItem>
                ))}
                <DropdownMenuItem asChild className="uppercase tracking-wider">
                  {signedIn ? <Link href={consoleHref}>Console</Link> : <a href={consoleHref}>Console</a>}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          <div
            ref={railContentRef}
            style={{ top: `${railStickyTop}px` }}
            className="sticky mt-4 flex flex-col"
          >
            <nav aria-label="Book" className="flex flex-col">
              <DocsTreeNodes nodes={tree.children} pathname={pathname} />
            </nav>

            <div className="flex flex-col gap-6 pt-12">
              {metaLinks.length > 0 && (
                <nav aria-label="Secondary" className="flex flex-col gap-2 font-mono text-sm">
                  {metaLinks.map((link) => (
                    <SiteLinkItem key={link.href} link={link} />
                  ))}
                </nav>
              )}
              <div className="flex flex-col gap-4">
                <div className="flex flex-col items-start gap-2">
                  <ThemeToggle />
                  <CornersToggle />
                </div>
                <p className="text-xs text-muted-foreground">
                  © {new Date().getFullYear()} {companyName}
                </p>
              </div>
            </div>
          </div>
        </aside>

        {/* Center + Right columns rendered by DocsPageView */}
        <div className="flex min-w-0 flex-1 flex-col">
          {children}

          <footer className={cn("flex flex-col gap-6 py-8 lg:hidden", INSET)}>
            {metaLinks.length > 0 && (
              <nav aria-label="Footer" className="flex flex-col gap-2 font-mono text-sm">
                {metaLinks.map((link) => (
                  <SiteLinkItem key={link.href} link={link} />
                ))}
              </nav>
            )}
            <div className="flex flex-col gap-4">
              <div className="flex flex-col items-start gap-2">
                <ThemeToggle />
                <CornersToggle />
              </div>
              <p className="text-xs text-muted-foreground">
                © {new Date().getFullYear()} {companyName}
              </p>
            </div>
          </footer>
        </div>
      </div>
    </div>
  );
}

function DocsRightRail({
  toc,
  repoUrl,
}: {
  toc: TOCItemType[];
  repoUrl?: string | null;
}) {
  const { setOpenSearch } = useSearchContext();
  const isMac = useIsMac();
  const activeAnchor = useActiveAnchor();

  return (
    <aside
      aria-label="On this page"
      className="hidden w-56 shrink-0 flex-col py-8 pr-8 pl-4 lg:flex xl:w-64"
    >
      <div className="sticky top-8 flex flex-col">
        {/* Top-right GitHub icon aligned with the mark and DOCS on the left */}
        <div className="flex h-6 items-center justify-end">
          {repoUrl && (
            <a
              href={repoUrl}
              target="_blank"
              rel="noreferrer"
              aria-label="GitHub repository"
              className="flex h-6 w-6 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
            >
              <svg className="size-5 fill-current" viewBox="0 0 24 24" aria-hidden="true">
                <path
                  fillRule="evenodd"
                  clipRule="evenodd"
                  d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.53 1.032 1.53 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"
                />
              </svg>
            </a>
          )}
        </div>

        {/* Search trigger aligned with <h1> title and first [-] item */}
        <button
          type="button"
          onClick={() => setOpenSearch(true)}
          className="mt-8 flex w-full cursor-pointer items-center justify-between rounded-md bg-muted/70 px-3.5 py-2 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <span>Search</span>
          <span className="text-muted-foreground/70">{isMac ? "⌘K" : "Ctrl K"}</span>
        </button>

        {/* Table of Contents */}
        {toc.length > 0 && (
          <div className="mt-8 flex flex-col">
            <p className="mb-3 font-mono text-sm text-muted-foreground">Contents</p>
            <nav aria-label="Table of contents" className="flex flex-col text-sm">
              {toc.map((item) => {
                const id = item.url.replace(/^#/, "");
                const isActive = activeAnchor === id;
                return (
                  <a
                    key={item.url}
                    href={item.url}
                    className={cn(
                      "border-l-2 py-1.5 transition-colors",
                      item.depth > 2 ? "pl-6" : "pl-3",
                      isActive
                        ? "border-foreground font-medium text-foreground"
                        : "border-transparent text-muted-foreground hover:text-foreground",
                    )}
                  >
                    {item.title}
                  </a>
                );
              })}
            </nav>
          </div>
        )}
      </div>
    </aside>
  );
}

export function DocsPageView({
  title,
  description,
  toc,
  neighbours,
  repoUrl,
  children,
}: {
  title: string;
  description?: string;
  toc: TOCItemType[];
  neighbours: { previous?: PageTree.Item; next?: PageTree.Item };
  repoUrl?: string | null;
  children: React.ReactNode;
}) {
  return (
    <AnchorProvider toc={toc}>
      <div className="flex flex-1 flex-col lg:flex-row">
        <article className="min-w-0 flex-1">
          <PaperHero title={title}>
            {description && <p className={LEAD}>{description}</p>}
          </PaperHero>
          <div className={cn("pb-12 lg:pb-16", INSET)}>
            <div className={cn("prose max-w-none [&_h2]:border-none [&_h2]:text-base [&_h2]:font-medium [&_h2]:tracking-normal [&_h3]:border-none [&_h3]:text-sm [&_h3]:font-medium [&_h4]:text-sm [&_h4]:font-medium [&_a:not([data-card])]:font-normal [&_a:not([data-card])]:text-foreground [&_a:not([data-card])]:underline-offset-4 [&_a:not([data-card]):hover]:no-underline [&_a:not([data-card]):hover]:opacity-100 [&_:not(pre)>code]:border-none [&_:not(pre)>code]:bg-muted [&_:not(pre)>code]:px-1.5 [&_:not(pre)>code]:py-0.5 [&_:not(pre)>code]:font-mono [&_:not(pre)>code]:font-normal [&_:not(pre)>code]:before:content-none [&_:not(pre)>code]:after:content-none", BODY)}>
              {children}
            </div>

            {(neighbours.previous || neighbours.next) && (
              <nav
                aria-label="Pagination"
                className="mt-16 flex flex-wrap items-center justify-between gap-4 pt-8 text-sm"
              >
                {neighbours.previous ? (
                  <Link
                    href={neighbours.previous.url}
                    className="text-muted-foreground transition-colors hover:text-foreground"
                  >
                    <span className="text-muted-foreground/70">← Previous</span>{" "}
                    <span className="text-foreground">{neighbours.previous.name}</span>
                  </Link>
                ) : (
                  <span />
                )}
                {neighbours.next ? (
                  <Link
                    href={neighbours.next.url}
                    className="text-muted-foreground transition-colors hover:text-foreground"
                  >
                    <span className="text-muted-foreground/70">Next</span>{" "}
                    <span className="text-foreground">{neighbours.next.name} →</span>
                  </Link>
                ) : (
                  <span />
                )}
              </nav>
            )}
          </div>
        </article>

        <DocsRightRail toc={toc} repoUrl={repoUrl} />
      </div>
    </AnchorProvider>
  );
}
