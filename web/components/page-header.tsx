"use client";

import { useEffect, type ReactNode } from "react";
import { InfoIcon } from "lucide-react";
import { usePathname } from "next/navigation";

import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";

import { usePageHeaderSet } from "./page-header-context";
import { recordVisit } from "@/lib/search-recent";

export function PageHeader({
  title,
  count,
  info,
  action,
  backHref,
  backLabel,
}: {
  title?: string;
  /** How many the page lists, beside its title, as the Projects page counts
   *  its projects; none while that count cannot be read. */
  count?: number;
  /** What the page is, behind an ⓘ beside its title, as Langfuse's tracing
   *  page says what it lists: for a page whose toolbar leaves no room for a
   *  sentence. */
  info?: string;
  action?: ReactNode;
  backHref?: string;
  backLabel?: string;
}) {
  const set = usePageHeaderSet();
  const pathname = usePathname();
  useEffect(() => {
    const my = { backHref, backLabel };
    set(my);
    // Not the tab's title, which is each page's `metadata` alone: written here
    // as well, it was one more change after Next's own.
    if (title) recordVisit({ href: pathname, label: title });
    return () => set((cur) => (cur === my ? null : cur));
  }, [title, backHref, backLabel, set, pathname]);

  if (!title && !action) return null;
  // As tall as a page action, 36px, so a page with one and a page without
  // start their content at the same height and nothing jumps between them.
  return (
    <div className="flex min-h-9 w-full min-w-0 items-center gap-3">
      {title && (
        <div className="flex min-w-0 items-center gap-2">
          <h1
            tabIndex={-1}
            className="min-w-0 truncate text-base font-medium outline-none"
          >
            {title}
          </h1>
          {count !== undefined && (
            <Badge variant="secondary" className="shrink-0 text-xs">
              {count}
            </Badge>
          )}
          {info && (
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    className="shrink-0 cursor-help rounded-sm text-muted-foreground hover:text-foreground"
                    aria-label={`About ${title}`}
                  >
                    <InfoIcon className="size-4" />
                  </button>
                </TooltipTrigger>
                <TooltipContent className="max-w-xs">{info}</TooltipContent>
              </Tooltip>
            </TooltipProvider>
          )}
        </div>
      )}
      {action && <div className="ml-auto shrink-0">{action}</div>}
    </div>
  );
}
