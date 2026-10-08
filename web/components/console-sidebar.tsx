"use client";

import { type KeyboardEvent } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { buildConsoleNav, consolePlace, type ConsoleNavItem } from "@/lib/console-nav";
import { resolveReturnUrl } from "@/lib/console-trail";
import { PRODUCT_NAME } from "@/lib/site";
import { useFlags } from "@/lib/store";
import { useConsoleTrail, useLiveResources } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";
import { useSidebar } from "./sidebar-context";

function NavRow({
  item,
  showLabel,
  step = false,
  className,
}: {
  item: ConsoleNavItem;
  showLabel: boolean;
  step?: boolean;
  className?: string;
}) {
  const onClick = () => {
    if (item.title && !step) {
      document.title = `${item.title} · ${PRODUCT_NAME}`;
    }
  };
  // Closed, a row is an icon button in the header's hamburger's 32 × 32 box,
  // and the rail is one button wide, so the two share a column.
  if (!showLabel) {
    return (
      <Link
        href={item.url}
        onClick={onClick}
        aria-label={item.title}
        title={item.title}
        aria-current={item.isActive ? "page" : undefined}
        {...(step ? { ["data-rail-step"]: "" } : {})}
        className={cn(
          "flex size-8 items-center justify-center rounded-md [a+&]:mt-1",
          item.isActive
            ? "bg-console-accent-tint text-console-accent-strong"
            : "text-muted-foreground hover:bg-sidebar-accent",
          className,
        )}
      >
        <item.icon className="size-4" />
      </Link>
    );
  }
  const rowClass = cn(
    "flex h-8 items-center rounded-md text-sm [a+&]:mt-1",
    item.isActive
      ? "bg-console-accent-tint font-medium text-console-accent-strong"
      : "text-foreground hover:bg-sidebar-accent",
    className,
  );
  const icon = (
    <span className="flex w-11 shrink-0 justify-center">
      <item.icon
        className={cn("size-4", !item.isActive && "text-muted-foreground")}
      />
    </span>
  );
  const label = (
    <span className="flex flex-1 items-center min-w-0 overflow-hidden pr-1">
      <span className="whitespace-nowrap truncate">{item.title}</span>
    </span>
  );
  return (
    <Link
      href={item.url}
      onClick={onClick}
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

export function ConsoleSidebar() {
  const pathname = usePathname();
  const { collapsed, toggle, closeOnMobile, animating } = useSidebar();
  const expanded = !collapsed;

  const trail = useConsoleTrail();

  const flags = useFlags();
  const groups = buildConsoleNav(pathname, flags);
  const steppedIn = groups[0]?.title === "Account";
  const { live, fallback } = useLiveResources();
  const returnUrl = resolveReturnUrl(trail, live, fallback);
  const returnLabel =
    consolePlace(returnUrl)?.kind === "organization"
      ? "Back to organization"
      : "Back to project";

  // ⚠ **A page that is not found has no rail of its own.** The rail is spelled
  // from the path, and a path that names nothing spells a rail of links to
  // more "not found". `(app)/not-found.tsx` marks itself, and every row here
  // answers the mark in CSS: the address's rows go, and the way back, which
  // Account draws anyway, comes. In CSS because only the page knows — an
  // outage draws the same empty store a dead address does — and the page
  // renders after the rail: a class is right in the server's HTML, where
  // state would correct itself once the console had hydrated.
  //
  // The rail is a column under the header's first cell: the same width, and
  // from `md` the same rule on its right, flush with the window's edge. Its
  // rows keep one inset, `px-3.5`, so closed, where the column is one 32px
  // button with that inset on each side, every icon lands on the centre the
  // header's mark stands on. The control that opens and closes it is the
  // header's, in the cell above; closed, the whole column is a way to open
  // it too — the pointer says so, and a press anywhere off a row opens it —
  // so nobody has to aim at the one button. Below `md` it is a drawer under
  // the header, opened from the header's hamburger.
  return (
    <aside
      id="console-sidebar"
      className={cn(
        "flex min-h-0 flex-col overflow-hidden bg-sidebar",
        "fixed top-15 bottom-0 left-0 z-40 w-72",
        collapsed ? "-translate-x-full md:cursor-pointer" : "translate-x-0",
        "md:static md:shrink-0 md:translate-x-0 md:border-r md:border-sidebar-border",
        collapsed ? "md:w-15" : "md:w-64",
        animating ? "transition-transform duration-200 md:transition-none" : "transition-none",
      )}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest("a[href]")) {
          closeOnMobile();
        } else if (collapsed) {
          toggle();
        }
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
        <nav aria-label="Main" onKeyDown={onNavKeyDown} className="px-3.5 py-3.5">
          {groups.map((group, i) => (
            <div
              key={group.title}
              className={cn(expanded && i > 0 && "mt-6")}
            >
              {/* A group's title stands a row's height clear of the group
                  above and a half-row over its own first row, on the icons'
                  left edge (`w-11` centres a 16px icon at 22px). */}
              {expanded ? (
                <div className="flex pb-2 pl-3.5 text-xs font-medium text-muted-foreground group-has-[[data-console-not-found]]/console:hidden">
                  <span className="whitespace-nowrap">{group.title}</span>
                </div>
              ) : (
                i > 0 && <div className="my-2 border-t border-sidebar-border" />
              )}
              {group.items.map((item) => (
                <NavRow
                  key={item.title}
                  item={item}
                  showLabel={expanded}
                  className="group-has-[[data-console-not-found]]/console:hidden"
                />
              ))}
              <NavRow
                item={{ title: returnLabel, url: returnUrl, icon: ArrowLeft, isActive: false }}
                showLabel={expanded}
                step
                className={
                  steppedIn
                    ? undefined
                    : "hidden group-has-[[data-console-not-found]]/console:flex"
                }
              />
            </div>
          ))}
        </nav>
      </ScrollArea>
    </aside>
  );
}
