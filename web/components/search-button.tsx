"use client";

import { Search } from "lucide-react";

import { useConsoleUi } from "@/components/console-ui-context";
import { useIsMac } from "@/lib/use-platform";
import { cn } from "@/lib/utils";

export function SearchButton() {
  const isMac = useIsMac();
  const { openSearch } = useConsoleUi();
  const chord = isMac ? "⌘K" : "Ctrl K";
  return (
    <button
      type="button"
      data-testid="search-button"
      aria-label="Search"
      aria-keyshortcuts={isMac ? "Meta+K" : "Control+K"}
      title={`Search (${chord})`}
      onClick={openSearch}
      className={cn(
        "relative flex h-8 w-11 shrink-0 items-center justify-center",
        "rounded-menu text-muted-foreground hover:text-foreground",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring",
        "hover:bg-sidebar-accent",
      )}
    >
      <Search className="size-4" />
    </button>
  );
}
