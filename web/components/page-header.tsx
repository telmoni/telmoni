"use client";

import { useEffect, type ReactNode } from "react";
import { usePageHeaderSet } from "./page-header-context";
import { recordVisit } from "@/lib/search-recent";
import { PRODUCT_NAME } from "@/lib/site";

export function PageHeader({
  title,
  action,
  backHref,
  backLabel,
}: {
  title?: string;
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
  return (
    <div className="flex min-h-8 w-full min-w-0 items-center gap-3">
      {title && (
        <h1
          tabIndex={-1}
          className="min-w-0 truncate text-base font-medium outline-none"
        >
          {title}
        </h1>
      )}
      {action && <div className="ml-auto shrink-0">{action}</div>}
    </div>
  );
}
