"use client";

import { ArrowLeft, Menu } from "lucide-react";
import Link from "next/link";

import { AgentButton } from "./agent-button";
import { ResourceSelector } from "./resource-selector";
import { NavUser } from "./nav-user";
import { NotificationsBell } from "./notifications-bell";
import { SearchButton } from "./search-button";
import { usePageHeaderValue } from "./page-header-context";
import { useSidebar } from "./sidebar-context";
import { PRODUCT_NAME } from "@/lib/site";
import { cn } from "@/lib/utils";

export function ConsoleHeader() {
  const header = usePageHeaderValue();
  const { collapsed, toggle } = useSidebar();

  const back =
    header?.backHref && header.backLabel
      ? { href: header.backHref, label: header.backLabel }
      : null;
  // The controls are the closed sidebar's icon buttons (32 × 32), and `py-3.5`
  // is the sidebar's 14px either side of its column, turned vertical.
  return (
    <header className="relative z-50 flex shrink-0 items-center gap-3.5 bg-background px-3.5 py-3.5 md:gap-3">
      <div className="flex min-w-0 items-center gap-3.5 md:flex-1">
        <div
          className={cn(
            "flex shrink-0 items-center justify-between",
            collapsed ? "md:w-8" : "md:w-64",
          )}
        >
          {!collapsed && (
            <span className="hidden min-w-0 shrink-0 self-center truncate text-[20px] leading-none font-bold tracking-wider uppercase md:block">
              {PRODUCT_NAME}
            </span>
          )}
          <button
            type="button"
            onClick={toggle}
            aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
            aria-expanded={!collapsed}
            aria-controls="console-sidebar"
            title={collapsed ? "Expand navigation" : "Collapse navigation"}
            className="group flex size-8 shrink-0 cursor-pointer text-foreground"
          >
            <span className="flex flex-1 items-center justify-center rounded-menu text-muted-foreground group-hover:bg-sidebar-accent">
              <Menu className="size-4" />
            </span>
          </button>
        </div>
        {back && (
          <Link
            href={back.href}
            className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
          >
            <ArrowLeft className="size-4" />
            {back.label}
          </Link>
        )}
        <ResourceSelector />
      </div>
      <div className="ml-auto flex shrink-0 items-center justify-end gap-3">
        <SearchButton />
        <AgentButton />
        <NotificationsBell />
        <NavUser />
      </div>
    </header>
  );
}
