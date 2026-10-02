"use client";

import { type KeyboardEvent } from "react";
import Link from "next/link";
import { usePathname, useSelectedLayoutSegment } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import { ScrollArea } from "@/components/ui/scroll-area";
import {
  buildConsoleNav,
  isOrganizationSegment,
  rootSegment,
  type ConsoleNavItem,
} from "@/lib/console-nav";
import { resolveReturnUrl } from "@/lib/console-trail";
import { PRODUCT_NAME } from "@/lib/site";
import { useFlags, useProjects } from "@/lib/store";
import { useConsoleTrail } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";
import { useSidebar } from "./sidebar-context";

function NavRow({
  item,
  showLabel,
  step = false,
}: {
  item: ConsoleNavItem;
  showLabel: boolean;
  step?: boolean;
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
          "flex size-8 items-center justify-center rounded-menu [a+&]:mt-1",
          item.isActive
            ? "bg-console-accent-tint text-console-accent-strong"
            : "text-muted-foreground hover:bg-sidebar-accent",
        )}
      >
        <item.icon className="size-4" />
      </Link>
    );
  }
  const rowClass = cn(
    "flex h-8 items-center rounded-menu text-sm [a+&]:mt-1",
    item.isActive
      ? "bg-console-accent-tint font-medium text-console-accent-strong"
      : "text-foreground hover:bg-sidebar-accent",
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
  const rows = Array.from(
    e.currentTarget.querySelectorAll<HTMLElement>("a[href]"),
  );
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
  const { collapsed, closeOnMobile, animating } = useSidebar();
  const expanded = !collapsed;

  const trail = useConsoleTrail();

  const flags = useFlags();
  const projects = useProjects();
  const segment = useSelectedLayoutSegment() ?? "";
  const groups = buildConsoleNav(pathname, segment, flags);
  const steppedIn = groups[0]?.title === "Account";
  const returnProject = projects.find((p) => p.id !== "organization" && p.id !== "org") ?? projects[0];
  const returnUrl = resolveReturnUrl(
    trail,
    projects.map((t) => t.id),
    returnProject ? `/${returnProject.id}` : "/console",
  );
  const returnLabel = isOrganizationSegment(rootSegment(returnUrl))
    ? "Back to organization"
    : "Back to project";

  return (
    <aside
      id="console-sidebar"
      className={cn(
        "flex min-h-0 flex-col overflow-hidden bg-sidebar px-3.5 md:px-0",
        "fixed top-15 bottom-0 left-0 z-40 w-71",
        "rounded-r-menu md:rounded-none",
        collapsed ? "-translate-x-full" : "translate-x-0",
        "md:static md:shrink-0 md:translate-x-0",
        collapsed ? "md:w-8" : "md:w-64",
        animating ? "transition-transform duration-200 md:transition-none" : "transition-none",
      )}
      onClick={(e) => {
        const link = (e.target as HTMLElement).closest("a[href]");
        if (link && !link.hasAttribute("a[href]")) closeOnMobile();
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
        <nav aria-label="Main" onKeyDown={onNavKeyDown}>
          {groups.map((group, i) => (
            <div
              key={group.title}
              className={cn(expanded && i > 0 && "mt-4")}
            >
              {expanded ? (
                <div className="flex pb-1 pl-3.5 text-xs font-medium text-muted-foreground">
                  <span className="whitespace-nowrap">{group.title}</span>
                </div>
              ) : (
                i > 0 && <div className="mx-2.5 my-2 border-t border-sidebar-border" />
              )}
              {group.items.map((item) => (
                <NavRow
                  key={item.title}
                  item={item}
                  showLabel={expanded}
                />
              ))}
              {steppedIn && (
                <NavRow
                  item={{ title: returnLabel, url: returnUrl, icon: ArrowLeft, isActive: false }}
                  showLabel={expanded}
                  step
                />
              )}
            </div>
          ))}
        </nav>
      </ScrollArea>
    </aside>
  );
}
