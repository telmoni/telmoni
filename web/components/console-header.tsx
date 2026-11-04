"use client";

import { Menu, PanelLeft } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { AgentButton } from "./agent-button";
import { ResourceSelector } from "./resource-selector";
import { NavUser } from "./nav-user";
import { NotificationsBell } from "./notifications-bell";
import { SearchButton } from "./search-button";
import { useSidebar } from "./sidebar-context";
import { TelmoniWordmark } from "./telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";
import { organizationPath } from "@/lib/slug";
import { useActiveOrganization } from "@/lib/store";
import { cn } from "@/lib/utils";

// The bar across the top, in three cells. The first is the rail's column —
// the rail's width and, from `md`, the rail's rule on its right, which the
// rail continues below so one line runs from the top of the window to the
// bottom — and holds the rail's toggle and the brand. In Claude style, the
// control that toggles the rail stands at the left — centered on the rail's
// axis, in line with the nav icons below — followed by the wordmark on
// the right when open, column-aligned with the nav row labels at the 48px mark.
// The button's padding is compact (`size-7`), leaving clean breathing space
// before the wordmark. Closed, the rail's column holds the toggle button alone
// on that same center axis. Below `md` the rail is a drawer, so the cell
// holds a hamburger there. The second cell is where you stand; the third, the
// search and the account's buttons.
export function ConsoleHeader({
  organizationBadge,
}: {
  /// Drawn beside the organization's name in the switcher: a console built
  /// on this one puts the organization's plan there.
  organizationBadge?: React.ReactNode;
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
      className="relative z-50 flex h-13 shrink-0 items-stretch border-b border-border bg-background"
    >
      <div
        className={cn(
          "flex shrink-0 items-center gap-2 px-gutter md:border-r md:border-sidebar-border",
          collapsed
            ? "md:w-13 md:cursor-expand"
            : "md:w-sidebar",
        )}
        onClick={(e) => {
          if (collapsed && !(e.target as HTMLElement).closest("a[href], button")) toggle();
        }}
      >
        <button
          type="button"
          onClick={toggle}
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
          aria-controls="console-sidebar"
          className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground md:hidden"
        >
          <Menu className="size-4" />
        </button>
        <button
          type="button"
          onClick={toggle}
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
          aria-controls="console-sidebar"
          title={collapsed ? "Open sidebar" : "Close sidebar"}
          className="hidden size-7 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground md:flex"
        >
          <PanelLeft className="size-4" />
        </button>
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
            "hidden min-w-0 items-center hover:opacity-70 md:flex",
            collapsed && "md:hidden",
          )}
        >
          {/* Claude style: the clean wordmark stands alone beside the toggle button,
              aligned with the navigation link text at the 48px column. */}
          <TelmoniWordmark className="h-4 shrink-0" />
        </Link>
      </div>
      {/* `pl-1` with the switcher's own `px-2`: its name starts where the
          page's title does, a gutter from the rail's rule (`console-shell.tsx`). */}
      <div className="flex min-w-0 flex-1 items-center gap-gutter pl-1 pr-gutter">
        <ResourceSelector organizationBadge={organizationBadge} />
      </div>
      <div className="flex shrink-0 items-center justify-end gap-gutter pr-gutter">
        <SearchButton />
        <AgentButton />
        <NotificationsBell />
        <NavUser />
      </div>
    </header>
  );
}
