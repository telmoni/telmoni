"use client";

import { useEffect, type ReactNode } from "react";
import { Badge } from "@/components/ui/badge";

import { usePageHeaderSet } from "./page-header-context";
import { recordVisit } from "@/lib/search-recent";
import { PRODUCT_NAME } from "@/lib/site";

export function PageHeader({
  title,
  count,
  action,
  backHref,
  backLabel,
}: {
  title?: string;
  /** How many the page lists, beside its title, as the Projects page counts
   *  its projects; none while that count cannot be read. */
  count?: number;
  action?: ReactNode;
  backHref?: string;
  backLabel?: string;
}) {
  const set = usePageHeaderSet();
  useEffect(() => {
    const my = { backHref, backLabel };
    set(my);
    if (title) {
      document.title = `${title} · ${PRODUCT_NAME}`;
      recordVisit({ href: window.location.pathname, label: title });
    }
    return () => set((cur) => (cur === my ? null : cur));
  }, [title, backHref, backLabel, set]);

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
        </div>
      )}
      {action && <div className="ml-auto shrink-0">{action}</div>}
    </div>
  );
}
