import type { ReactNode } from "react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function Rows({
  children,
  className,
  flush,
}: {
  children: ReactNode;
  className?: string;
  /** Inside a card already, a settings `Section`'s: the rows alone, ruled
   *  apart, with no card of their own. */
  flush?: boolean;
}) {
  if (flush) {
    return <div className={cn("-my-gutter divide-y divide-border", className)}>{children}</div>;
  }
  return (
    <Card className={cn("gap-0 divide-y divide-border py-0 text-sm", className)}>
      {children}
    </Card>
  );
}

export function Row({
  label,
  hint,
  children,
  danger,
  keep,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children?: ReactNode;
  danger?: boolean;
  /** The value keeps its width, a figure the hint wraps beside rather than
   *  one broken over lines. */
  keep?: boolean;
}) {
  return (
    <div className="grid min-h-14 grid-cols-1 items-center gap-gutter py-gutter sm:grid-cols-[minmax(0,1fr)_auto]">
      <div className="grid min-w-0 gap-0.5">
        <span className={cn("font-medium", danger && "text-destructive")}>{label}</span>
        {hint ? <span className="text-muted-foreground">{hint}</span> : null}
      </div>
      {children !== undefined && (
        <div
          className={cn(
            "flex max-w-full items-center justify-start gap-2 text-left sm:justify-end sm:text-right",
            keep ? "shrink-0 whitespace-nowrap" : "min-w-0",
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}
