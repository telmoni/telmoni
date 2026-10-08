"use client";

import { Search } from "lucide-react";

import { useConsoleUi } from "@/components/console-ui-context";
import { useIsMac } from "@/lib/use-platform";
import { cn } from "@/lib/utils";

// The way into the search, in the header beside the account's buttons; the
// chord opens the same dialog (`console-search.tsx`) from anywhere.
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
        "flex size-8 shrink-0 items-center justify-center",
        "rounded-md text-muted-foreground hover:text-foreground",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring",
        "hover:bg-sidebar-accent",
      )}
    >
      <Search className="size-4" />
    </button>
  );
}
