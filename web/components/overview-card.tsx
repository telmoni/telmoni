import type { LucideIcon } from "lucide-react";
import { ArrowRight } from "lucide-react";
import Link from "next/link";
import { Children, type ReactNode } from "react";

import { Card } from "@/components/ui/card";

/** An overview's cards in three columns from `lg`, dealt across in order and each stacked
 *  a gutter under the one above at its own height; below `lg`, one column in that order. */
export function OverviewGrid({ children }: { children: ReactNode }) {
  const cards = Children.toArray(children);
  const stacks = [0, 1, 2].map((column) => cards.filter((_, i) => i % 3 === column));
  return (
    <div className="grid grid-cols-1 items-start gap-gutter lg:grid-cols-3">
      {stacks.map((stack, column) => (
        // Below `lg` a stack steps aside, its cards the grid's own, in their order again.
        <div key={column} className="contents lg:flex lg:flex-col lg:gap-gutter">
          {stack.map((card, row) => (
            <div key={row} className="min-w-0" style={{ order: row * 3 + column }}>
              {card}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

export type CardLink = { href: string; label: string };

/** One card of an overview: its title and icon, what it shows, and where it leads, each a
 *  ruled row at its foot. */
export function OverviewCard({
  title,
  icon: Icon,
  action,
  links = [],
  children,
}: {
  title: string;
  icon?: LucideIcon;
  /** A control beside the title, such as Mark read. */
  action?: ReactNode;
  links?: readonly CardLink[];
  children: ReactNode;
}) {
  // Titles are unique on a page, so each names its own heading's id.
  const heading = `card-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
  return (
    <Card className="gap-0 overflow-hidden p-0" role="region" aria-labelledby={heading}>
      <div className="flex items-center justify-between gap-gutter p-gutter">
        <h2 id={heading} className="flex min-w-0 items-center gap-gutter text-base font-medium">
          {Icon && <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />}
          <span className="truncate">{title}</span>
        </h2>
        {action}
      </div>
      <div className="grid min-w-0 gap-gutter px-gutter pb-gutter text-sm">{children}</div>
      {links.map((link) => (
        <Link
          key={link.href}
          href={link.href}
          className="flex items-center gap-gutter border-t border-border p-gutter text-sm transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
        >
          <ArrowRight className="size-4 shrink-0" aria-hidden="true" />
          {link.label}
        </Link>
      ))}
    </Card>
  );
}

/** A figure on a card: its value large, what it counts under it. */
export function Figure({ value, label }: { value: ReactNode; label: string }) {
  return (
    <div className="min-w-0">
      <p className="truncate text-xl font-medium tabular-nums">{value}</p>
      <p className="text-xs text-muted-foreground">{label}</p>
    </div>
  );
}
