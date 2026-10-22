"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { ArrowLeft, InfoIcon } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";

import { recordVisit } from "@/lib/search-recent";
import { useUser } from "@/lib/store";

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
  /** Where the page's way back leads, drawn above its title as `← backLabel`:
   *  a page of one trace or one session, back to its list. Both or none. */
  backHref?: string;
  backLabel?: string;
}) {
  const pathname = usePathname();
  const owner = useUser()?.id;
  useEffect(() => {
    // Not the tab's title, which is each page's `metadata` alone: written here
    // as well, it was one more change after Next's own.
    if (title && owner) recordVisit(owner, { href: pathname, label: title });
  }, [title, pathname, owner]);

  // A page that came after its loading state finds the focus where the
  // console shell parked it, on the main region, as no heading stood there
  // yet (`console-shell.tsx`): the heading takes it, as it would have had the
  // page come at once. Focus anywhere else is the person's, and stays.
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (document.activeElement?.id === "console-main") {
      heading.current?.focus({ preventScroll: true });
    }
  }, []);

  if (!title && !action) return null;
  // As tall as a page action, 36px, so a page with one and a page without
  // start their content at the same height and nothing jumps between them.
  const row = (
    <div className="flex min-h-9 w-full min-w-0 items-center gap-3">
      {title && (
        <div className="flex min-w-0 items-center gap-2">
          <h1
            ref={heading}
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
  if (!backHref || !backLabel) return row;
  // In the page, over its title, rather than in the console's header: the way
  // back belongs to the page it leaves, and the header is the console's own.
  return (
    <div className="grid w-full min-w-0 gap-2">
      <Link
        href={backHref}
        className="flex w-fit items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" />
        {backLabel}
      </Link>
      {row}
    </div>
  );
}
