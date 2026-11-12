"use client";

import { ListIndentDecrease, ListIndentIncrease } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { AgentButton } from "./agent-button";
import { ResourceSelector } from "./resource-selector";
import { NavUser } from "./nav-user";
import { NotificationsBell } from "./notifications-bell";
import { SearchButton } from "./search-button";
import { useSidebar } from "./sidebar-context";
import { TelmoniMark, TelmoniWordmark } from "./telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";
import { organizationPath } from "@/lib/slug";
import { useActiveOrganization } from "@/lib/store";
import { cn } from "@/lib/utils";

// The brand opens the compact rail; expanded controls stand beside it.
export function ConsoleHeader({
  organizationBadges,
}: {
  /// Drawn beside each organization's name in the switcher, by its id: a
  /// console built on this one puts the organization's plan there.
  organizationBadges?: Readonly<Record<string, React.ReactNode>>;
}) {
  const { collapsed, toggle } = useSidebar();
  const organization = useActiveOrganization();
  const pathname = usePathname();

  // Where `/console` would send you — the overview of the organization the
  // console stands in, or the account's notifications in none — linked
  // straight, without the redirect's round trip.
  const home = organization ? organizationPath(organization.slug) : "/settings/notifications";

  return (
    <header
      data-slot="console-header"
      className="relative z-50 flex h-console-header shrink-0 items-stretch border-b border-border bg-background"
    >
      <div
        className={cn(
          "group/brand relative flex shrink-0 items-center gap-gutter px-page md:border-r md:border-sidebar-border md:bg-sidebar",
          collapsed
            ? "md:w-sidebar-collapsed md:cursor-expand"
            : "md:w-sidebar",
        )}
        onClick={(e) => {
          if (collapsed && !(e.target as HTMLElement).closest("a[href], button")) toggle();
        }}
      >
        <span
          data-slot="console-brand-mark"
          aria-hidden="true"
          className={cn(
            "pointer-events-none absolute inset-y-0 left-page flex w-control items-center justify-center",
            collapsed && "md:group-has-[button:hover]/brand:opacity-0 md:group-has-[button:focus-visible]/brand:opacity-0",
          )}
        >
          <TelmoniMark />
        </span>
        <Link
          href={home}
          // Already there, a click would only run the page again — the
          // server's round trip, and the page flashing back — for nothing.
          // On the navigation rather than the click, so a ⌘- or Ctrl-click,
          // which Next leaves to the browser, still opens a new tab.
          onNavigate={(e) => {
            if (pathname === home) e.preventDefault();
          }}
          aria-label={PRODUCT_NAME}
          className={cn(
            "hidden min-w-0 shrink-0 items-center gap-2 transition-colors hover:text-muted-foreground md:flex",
            collapsed && "md:hidden",
          )}
        >
          <span aria-hidden="true" className="size-control shrink-0" />
          <TelmoniWordmark className="h-5 shrink-0" />
        </Link>
        <button
          type="button"
          onClick={toggle}
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
          aria-controls="console-sidebar"
          title={collapsed ? "Open sidebar" : "Close sidebar"}
          className={cn(
            "group/toggle ml-auto flex size-control shrink-0 cursor-pointer items-center justify-center rounded-md bg-transparent hover:bg-transparent active:bg-transparent md:hover:bg-sidebar-accent/50 md:active:bg-sidebar-accent/50",
            collapsed && "md:cursor-expand",
          )}
        >
          {collapsed && (
            <ListIndentIncrease
              aria-hidden="true"
              className="hidden size-icon text-muted-foreground opacity-0 group-hover/toggle:opacity-100 group-focus-visible/toggle:opacity-100 md:block"
            />
          )}
          <ListIndentDecrease
            aria-hidden="true"
            className={cn("hidden size-icon text-muted-foreground", !collapsed && "md:block")}
          />
        </button>
      </div>
      {/* The switcher's outlined edge stands on the page's line. */}
      <div className="flex min-w-0 flex-1 items-center gap-gutter pl-page pr-gutter">
        <ResourceSelector organizationBadges={organizationBadges} />
      </div>
      <div className="flex shrink-0 items-center justify-end gap-0.5 pr-page">
        <SearchButton />
        <AgentButton />
        <NotificationsBell />
        <NavUser />
      </div>
    </header>
  );
}
