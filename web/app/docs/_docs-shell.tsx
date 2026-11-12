"use client";

import type * as PageTree from "fumadocs-core/page-tree";
import { AnchorProvider, type TOCItemType, useActiveAnchor } from "fumadocs-core/toc";
import { useSearchContext } from "fumadocs-ui/contexts/search";
import { ArrowLeft, ArrowRight, ChevronDown, ChevronRight, ChevronUp } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useRef, useState } from "react";

import { GithubIcon } from "@/components/brand-icons";
import { CornersToggle } from "@/components/corners-toggle";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { BODY, LEAD, PaperHero } from "@/components/paper-page";
import { RailLink } from "@/components/rail-link";
import { RailShortcuts, type RailShortcut } from "@/components/rail-shortcuts";
import { type NavLink } from "@/components/site-nav";
import { TelmoniLockup } from "@/components/telmoni-mark";
import { ThemeToggle } from "@/components/theme-toggle";
import { INSET } from "@/lib/inset";
import { PRODUCT_NAME } from "@/lib/site";
import { useOverflow } from "@/lib/use-overflow";
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
    label: "Docs",
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
    label: "Self-host",
    href: "/docs/self-host/overview",
    match: (p: string) => p.startsWith("/docs/self-host") || p.startsWith("/docs/contributing"),
  },
] as const satisfies readonly DocsMenuSection[];

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
        "group flex items-baseline gap-2 py-1 text-sm transition-colors",
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
              className="pt-6 pb-1.5 text-xs uppercase tracking-wider text-muted-foreground/70"
            >
              {node.name}
            </p>
          );
        }
        if (node.type === "folder") {
          return (
            <div key={node.$id ?? `folder-${i}`} className="flex flex-col">
              {node.name && (
                <p className="pt-6 pb-1.5 text-xs uppercase tracking-wider text-muted-foreground/70">
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
  companyName,
  children,
}: {
  tree: PageTree.Root;
  signedIn: boolean;
  primaryLinks: NavLink[];
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
  const rail = useRef<HTMLDivElement>(null);
  const railOverflow = useOverflow(rail, "y");

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileMenuOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

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

        {/* Mobile top bar, framed as the splash's header is, pinned so the mark
            and the section menu stay put as the page scrolls. */}
        <header
          className={cn(
            "sticky top-0 z-40 flex h-15 items-center justify-between border-b border-border bg-background lg:hidden",
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
              className="inline-flex h-control shrink-0 cursor-pointer items-center whitespace-nowrap rounded-md bg-muted text-sm font-medium text-foreground transition-colors hover:bg-muted/80"
            >
              <span className="px-2">{activeSection.label}</span>
              <span className="flex h-full items-center border-l border-background px-1.5 text-muted-foreground">
                {mobileMenuOpen ? (
                  <ChevronUp className="size-icon" aria-hidden="true" />
                ) : (
                  <ChevronDown className="size-icon" aria-hidden="true" />
                )}
              </span>
            </button>
          </div>
          <button
            type="button"
            onClick={() => setOpenSearch(true)}
            className="inline-flex h-control cursor-pointer items-center rounded-md bg-muted/80 px-gutter text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
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
            <div className="flex flex-col gap-2 pt-4 text-sm">
              <p className="pb-1 text-xs uppercase tracking-wider text-muted-foreground/70">
                SITE
              </p>
              {primaryLinks.map((link) => (
                <RailLink
                  key={link.href}
                  link={link}
                  arrow="always"
                  className="py-0.5"
                  onDone={() => setMobileMenuOpen(false)}
                />
              ))}
              {signedIn ? (
                <RailLink
                  link={{ label: "Console", href: "/console" }}
                  className="py-0.5"
                  onDone={() => setMobileMenuOpen(false)}
                />
              ) : (
                <>
                  <RailLink link={{ label: "Sign in", href: "/auth/login" }} handler className="py-0.5" />
                  <RailLink link={{ label: "Sign up", href: "/auth/signup" }} handler className="py-0.5" />
                </>
              )}
            </div>
          </div>
        )}

        {/* Desktop Left Rail: Brand Mark + DOCS switcher + [ ] / [-] Docs Tree.
            Pinned to the window, its tree scrolled on its own under the brand,
            apart from the page, with no scrollbar: a bar between the rail and
            the article reads as a wall, so the end with more fades instead. */}
        <aside
          aria-label="Documentation navigation"
          className="sticky top-0 hidden h-svh w-52 shrink-0 flex-col lg:flex"
        >
          <div className="flex items-start gap-2.5 pt-8 pb-4 pl-8 pr-4">
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
                  className="inline-flex h-6 shrink-0 cursor-pointer items-center whitespace-nowrap rounded-md bg-muted text-sm font-medium text-foreground transition-colors hover:bg-muted/80"
                >
                  <span className="px-2">{activeSection.label}</span>
                  <span className="flex h-full items-center border-l border-background px-1.5 text-muted-foreground">
                    {switcherOpen ? (
                      <ChevronUp className="size-icon" aria-hidden="true" />
                    ) : (
                      <ChevronDown className="size-icon" aria-hidden="true" />
                    )}
                  </span>
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="min-w-40">
                {DOCS_SECTIONS.map((sec) => (
                  <DropdownMenuItem key={sec.id} asChild className="justify-between gap-6">
                    <Link href={sec.href}>
                      <span>{sec.label}</span>
                      {sec.id === activeSection.id && <ChevronRight aria-hidden="true" />}
                    </Link>
                  </DropdownMenuItem>
                ))}
                <DropdownMenuItem asChild>
                  {signedIn ? <Link href={consoleHref}>Console</Link> : <a href={consoleHref}>Console</a>}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          <div ref={rail} className="relative min-h-0 flex-1" data-overflow={railOverflow || undefined}>
            <div data-scroll-y="" className="h-full overflow-y-auto overscroll-contain pt-4 pb-8 pl-8 pr-4">
              <div className="flex flex-col">
                <nav aria-label="Book" className="flex flex-col">
                  <DocsTreeNodes nodes={tree.children} pathname={pathname} />
                </nav>

                <div className="flex flex-col gap-6 pt-12">
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
            </div>
          </div>
        </aside>

        {/* Center + Right columns rendered by DocsPageView */}
        <div className="flex min-w-0 flex-1 flex-col">
          {children}

          <footer className={cn("flex flex-col gap-6 py-8 lg:hidden", INSET)}>
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
              className="flex size-6 items-center justify-center text-muted-foreground transition-colors hover:text-foreground"
            >
              <GithubIcon className="size-6" />
            </a>
          )}
        </div>

        {/* Search trigger aligned with <h1> title and first [-] item */}
        <button
          type="button"
          onClick={() => setOpenSearch(true)}
          className="mt-8 flex h-control w-full cursor-pointer items-center justify-between rounded-md bg-muted/70 px-gutter text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <span>Search</span>
          <span className="text-muted-foreground/70">{isMac ? "⌘K" : "Ctrl K"}</span>
        </button>

        {/* Table of Contents */}
        {toc.length > 0 && (
          <div className="mt-8 flex flex-col">
            <p className="mb-3 text-sm text-muted-foreground">Contents</p>
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
            <div className={cn("prose max-w-none [&_h2]:border-none [&_h2]:text-base [&_h2]:font-medium [&_h2]:tracking-normal [&_h3]:border-none [&_h3]:text-sm [&_h3]:font-medium [&_h4]:text-sm [&_h4]:font-medium [&_a:not([data-card])]:font-normal [&_a:not([data-card])]:text-link [&_a:not([data-card])]:underline-offset-4 [&_a:not([data-card]):hover]:no-underline [&_a:not([data-card]):hover]:opacity-100 [&_:not(pre)>code]:border-none [&_:not(pre)>code]:bg-muted [&_:not(pre)>code]:px-1.5 [&_:not(pre)>code]:py-0.5 [&_:not(pre)>code]:font-mono [&_:not(pre)>code]:font-normal [&_:not(pre)>code]:before:content-none [&_:not(pre)>code]:after:content-none", BODY)}>
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
                    className="inline-flex items-center gap-1.5 text-muted-foreground transition-colors hover:text-foreground"
                  >
                    <ArrowLeft className="size-icon text-muted-foreground/70" aria-hidden="true" />
                    <span className="text-muted-foreground/70">Previous</span>
                    <span className="text-foreground">{neighbours.previous.name}</span>
                  </Link>
                ) : (
                  <span />
                )}
                {neighbours.next ? (
                  <Link
                    href={neighbours.next.url}
                    className="inline-flex items-center gap-1.5 text-muted-foreground transition-colors hover:text-foreground"
                  >
                    <span className="text-muted-foreground/70">Next</span>
                    <span className="text-foreground">{neighbours.next.name}</span>
                    <ArrowRight className="size-icon text-foreground" aria-hidden="true" />
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
