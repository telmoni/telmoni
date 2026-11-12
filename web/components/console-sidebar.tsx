"use client";

import { type KeyboardEvent } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import {
  buildConsoleNav,
  consolePlace,
  isAccountPath,
  navBlocks,
  type ConsoleNavItem,
} from "@/lib/console-nav";
import { resolveReturnUrl } from "@/lib/console-trail";
import { sequenceHint } from "@/lib/keys";
import { usePageCovered } from "@/lib/page-cover";
import { PRODUCT_NAME } from "@/lib/site";
import { useFlags } from "@/lib/store";
import { useLetterKeys } from "@/lib/use-accessibility";
import { useConsoleTrail, useLiveResources } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";
import { useSidebar } from "./sidebar-context";

function NavRow({
  item,
  showLabel,
  step = false,
  short,
  className,
}: {
  item: ConsoleNavItem;
  showLabel: boolean;
  step?: boolean;
  /** The row's words where its title runs longer than the rail is wide: the
   *  title stays its name, read out and shown under the pointer. */
  short?: string;
  className?: string;
}) {
  // The drawer closes on `onNavigate`, not a click: a ⌘-click to a new tab
  // navigates nothing here, so it leaves the drawer open.
  const { closeOnMobile } = useSidebar();
  const letterKeys = useLetterKeys();
  const covered = usePageCovered();
  // Its `g` sequence under the pointer, while letter keys work and no panel
  // over the page holds them off.
  const hint =
    letterKeys && !covered && item.key ? `${item.title} (${sequenceHint(item.key)})` : undefined;
  // No title is written here: the tab's title is each page's `metadata`, which
  // Next sets as the page lands. Written on the click as well, it went up
  // before the page came, and wrong on a project's first page: the row says
  // Agents where the page is titled by the project's name.
  if (!showLabel) {
    return (
      <Link
        href={item.url}
        onNavigate={closeOnMobile}
        aria-label={item.title}
        title={hint ?? item.title}
        aria-current={item.isActive ? "page" : undefined}
        {...(step ? { ["data-rail-step"]: "" } : {})}
        className={cn(
          "flex size-control items-center justify-center mx-auto rounded-md [a+&]:mt-0.5",
          item.isActive
            ? "bg-sidebar-accent text-sidebar-accent-foreground"
            : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground",
          className,
        )}
      >
        <item.icon className={cn("size-icon", item.isActive && "text-action")} />
      </Link>
    );
  }
  const rowClass = cn(
    "flex h-control items-center gap-gutter rounded-md px-gutter text-sm [a+&]:mt-0.5",
    item.isActive
      ? "bg-sidebar-accent text-sidebar-accent-foreground"
      : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground",
    className,
  );
  const icon = <item.icon className={cn("size-icon shrink-0", item.isActive && "text-action")} />;
  const label = (
    <span className="flex flex-1 items-center min-w-0 overflow-hidden pr-1">
      <span className="whitespace-nowrap truncate">{short ?? item.title}</span>
    </span>
  );
  return (
    <Link
      href={item.url}
      onNavigate={closeOnMobile}
      aria-label={short ? item.title : undefined}
      title={hint ?? (short ? item.title : undefined)}
      aria-current={item.isActive ? "page" : undefined}
      {...(step ? { ["data-rail-step"]: "" } : {})}
      className={rowClass}
    >
      {icon}
      {label}
    </Link>
  );
}

function onNavKeyDown(e: KeyboardEvent<HTMLElement>) {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  // Only the rows on screen: the ones the not-found mark hides are still in
  // the document, and an arrow key must not land on one.
  const rows = Array.from(
    e.currentTarget.querySelectorAll<HTMLElement>("a[href]"),
  ).filter((row) => row.getClientRects().length > 0);
  const from = (e.target as HTMLElement).closest<HTMLElement>("a[href]");
  const at = from ? rows.indexOf(from) : -1;
  if (at === -1) return;
  e.preventDefault();
  const to =
    e.key === "ArrowDown"
      ? Math.min(at + 1, rows.length - 1)
      : e.key === "ArrowUp"
        ? Math.max(at - 1, 0)
        : e.key === "Home"
          ? 0
          : rows.length - 1;
  rows[to]?.focus();
}

export function ConsoleSidebar({
  version,
}: {
  /// The release the console was built as (`lib/server/version.ts`), named at
  /// the rail's foot.
  version: string;
}) {
  const pathname = usePathname();
  const { collapsed, toggle, animating } = useSidebar();
  const expanded = !collapsed;

  const trail = useConsoleTrail();

  const flags = useFlags();
  const groups = buildConsoleNav(pathname, flags);
  const steppedIn = isAccountPath(pathname);
  const { live, fallback } = useLiveResources();
  const returnUrl = resolveReturnUrl(trail, live, fallback);
  const backToOrganization = consolePlace(returnUrl)?.kind === "organization";
  const returnLabel = backToOrganization ? "Back to organization" : "Back to project";
  // On the rail open, the arrow says "back" and the row names the place: at
  // the rail's width (`--spacing-sidebar`) "Back to organization" would be
  // cut short.
  const returnShort = backToOrganization ? "Organization" : "Project";

  // The not-found mark hides invalid links before hydration; the compact
  // rail and mobile drawer share the header's navigation toggle.
  return (
    <aside
      id="console-sidebar"
      className={cn(
        "flex min-h-0 flex-col overflow-hidden bg-sidebar",
        "fixed top-console-header bottom-0 left-0 z-40 w-72",
        // The drawer's edge, open only: closed, it waits just off the
        // screen's left edge, where a shadow cast right would show. Closed
        // below `md` it is hidden as well, or Tab would walk its rows unseen;
        // the visibility moves with the slide, so it goes once the slide ends.
        collapsed
          ? "-translate-x-full max-md:invisible md:cursor-expand"
          : "translate-x-0 shadow-drawer md:shadow-none",
        "md:static md:shrink-0 md:translate-x-0 md:border-r md:border-sidebar-border",
        collapsed ? "md:w-sidebar-collapsed" : "md:w-sidebar",
        // `translate`, the property Tailwind's `-translate-x-full` sets, not
        // `transform`; `visibility` beside it holds the drawer visible until
        // the slide ends.
        animating
          ? "transition-[translate,visibility] duration-200 md:transition-none"
          : "transition-none",
      )}
      onClick={(e) => {
        if (collapsed && !(e.target as HTMLElement).closest("a[href]")) toggle();
      }}
      aria-label="Console sidebar"
    >
      <ScrollArea
        type="scroll"
        className={cn(
          "min-h-0 flex-1 **:data-[slot=scroll-area-thumb]:bg-muted-foreground/30",
          collapsed && "**:data-[slot=scroll-area-scrollbar]:hidden",
        )}
      >
        <nav
          aria-label="Main"
          onKeyDown={onNavKeyDown}
          className="p-page"
        >
          {groups.map((group, i) => (
            <div
              key={group.title}
              className={cn(expanded && i > 0 && "mt-gutter")}
            >
              {/* The rows come in blocks (`navBlocks`), each parted from the one
                  above by a rule. The way back heads Settings' rail as a block
                  of its own, and is all a page not found shows. */}
              <NavRow
                item={{ title: returnLabel, url: returnUrl, icon: ArrowLeft, isActive: false }}
                short={returnShort}
                showLabel={expanded}
                step
                className={cn(!steppedIn && "hidden group-has-[[data-console-not-found]]/console:flex")}
              />
              {navBlocks(group.items).map((block, k) => (
                <div
                  key={k}
                  className={cn(k > 0 && "group-has-[[data-console-not-found]]/console:hidden")}
                >
                  {(k > 0 || steppedIn) && <div className="my-2 border-t border-sidebar-border" />}
                  {block.map((item) => (
                    <NavRow
                      key={item.title}
                      item={item}
                      showLabel={expanded}
                      className="group-has-[[data-console-not-found]]/console:hidden"
                    />
                  ))}
                </div>
              ))}
            </div>
          ))}
        </nav>
      </ScrollArea>
      {/* Match the header's inset in both rail states. */}
      <div className="flex h-console-header shrink-0 items-center border-t border-sidebar-border px-page">
        <div className={cn("flex h-control shrink-0 items-center", collapsed && "w-control justify-center")}>
          <p className="whitespace-nowrap text-xs text-muted-foreground tabular-nums">
            <span className="sr-only">
              {PRODUCT_NAME} version {version}
            </span>
            <span aria-hidden="true">v{version}</span>
          </p>
        </div>
      </div>
    </aside>
  );
}
