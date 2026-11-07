"use client";

import { Search } from "lucide-react";

import { useConsoleUi } from "@/components/console-ui-context";
import { Button } from "@/components/ui/button";
import { SEARCH_KEY } from "@/lib/keys";
import { useLetterKeys } from "@/lib/use-accessibility";
import { useIsMac } from "@/lib/use-platform";
import { cn } from "@/lib/utils";

// The way into the search, in the header beside the account's buttons; the
// chord opens the same dialog (`console-search.tsx`) from anywhere.
export function SearchButton() {
  const isMac = useIsMac();
  const letterKeys = useLetterKeys();
  const { searchOpen, setSearchOpen } = useConsoleUi();
  const chord = isMac ? "⌘K" : "Ctrl K";
  return (
    <Button
      variant="ghost"
      size="icon"
      type="button"
      data-testid="search-button"
      aria-label="Search"
      aria-expanded={searchOpen}
      aria-keyshortcuts={isMac ? "Meta+K" : "Control+K"}
      title={letterKeys ? `Search (${chord} or ${SEARCH_KEY})` : `Search (${chord})`}
      onClick={() => setSearchOpen(true)}
      className={cn(
        "flex size-control shrink-0 items-center justify-center",
        "rounded-md text-muted-foreground hover:text-foreground",
        "hover:bg-sidebar-accent/50",
        searchOpen && "bg-sidebar-accent text-foreground hover:bg-sidebar-accent",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring",
      )}
    >
      <Search className="size-4" />
    </Button>
  );
}
