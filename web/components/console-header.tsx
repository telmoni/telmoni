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
import { TelmoniLockup, TelmoniMark } from "./telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";
import { organizationPath } from "@/lib/slug";
import { useActiveOrganization } from "@/lib/store";
import { cn } from "@/lib/utils";

// The bar across the top, in three cells. The first is the rail's column —
// the rail's width and, from `md`, the rail's rule on its right, which the
// rail continues below so one line runs from the top of the window to the
// bottom — and holds the brand and the rail's toggle. Open, the mark and the
// name stand at the left and the control that closes the rail at the right.
// Closed, the mark alone stands on the column's centre, as every rail icon
// does — at the rail's own inset rather than centred in the cell, whose
// border would put it half a pixel off — and is itself the control that
// opens it: with the pointer anywhere in
// the cell, or focused, it shows the panel icon instead, and a press on the
// cell around it opens the rail as the rail's own space does below. Below
// `md` the rail is a drawer, so the cell holds a hamburger there. The second
// cell is where you stand; the third, the search and the account's buttons.
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
  const home = organization ? organizationPath(organization.slug) : "/account/notifications";

  return (
    <header
      data-slot="console-header"
      className="relative z-50 flex h-15 shrink-0 items-stretch border-b border-border bg-background"
    >
      <div
        className={cn(
          "flex shrink-0 items-center gap-3 px-gutter md:border-r md:border-sidebar-border",
          collapsed
            ? // A gutter in, as the closed rail's squares are: the 36px control
              // then draws its mark on the open lockup's pixel.
              "group/brand md:w-15 md:cursor-expand md:justify-start"
            : "md:w-sidebar md:justify-between",
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
          className="flex size-9 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground md:hidden"
        >
          <Menu className="size-4" />
        </button>
        {/* `pl-1.5`: open, the mark's left edge is 18px in — the cell's 12
            and 6 more — the same 18 the closed control gives it, so the mark
            rests on the rail's axis while the rail toggles, and only the name
            and the control come and go. The link's own box still opens on
            the rows' edge. Below `md` there is no brand at all: the bar is a
            phone's width, and the switcher's names need it more than the
            product's own does. */}
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
            "hidden min-w-0 items-center pl-1.5 hover:opacity-70 md:flex",
            collapsed && "md:hidden",
          )}
        >
          {/* 20px, under the pack's 24 for a header lockup: at 24 the word runs
              under the toggle in a 192px rail. */}
          <TelmoniLockup className="h-5 shrink-0" />
        </Link>
        {collapsed ? (
          <button
            type="button"
            onClick={toggle}
            aria-label="Expand navigation"
            aria-expanded={false}
            aria-controls="console-sidebar"
            title="Open sidebar"
            className="group relative hidden size-9 shrink-0 cursor-pointer items-center rounded-md pl-1.5 hover:bg-sidebar-accent/50 md:flex"
          >
            {/* The open lockup's mark exactly — its height, and its left edge
                at the same `pl-1.5` — so not a pixel of it changes when the
                rail closes; the control's icon is centred over it instead. */}
            <TelmoniMark className="h-5 w-auto group-hover/brand:hidden group-focus-visible:hidden" />
            <PanelLeft className="absolute inset-0 m-auto hidden size-4 text-muted-foreground group-hover/brand:block group-focus-visible:block" />
          </button>
        ) : (
          <button
            type="button"
            onClick={toggle}
            aria-label="Collapse navigation"
            aria-expanded={true}
            aria-controls="console-sidebar"
            title="Close sidebar"
            className="hidden size-9 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground md:flex"
          >
            <PanelLeft className="size-4" />
          </button>
        )}
      </div>
      {/* `pl-1` with the switcher's own `px-2`: its name starts where the
          page's title does, a gutter from the rail's rule — the rail's own inset
          (`console-shell.tsx`). */}
      <div className="flex min-w-0 flex-1 items-center gap-gutter pl-1 pr-gutter">
        <ResourceSelector organizationBadge={organizationBadge} />
      </div>
      <div className="flex shrink-0 items-center justify-end gap-3 pr-gutter">
        <SearchButton />
        <AgentButton />
        <NotificationsBell />
        <NavUser />
      </div>
    </header>
  );
}
