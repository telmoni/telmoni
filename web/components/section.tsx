import type { ReactNode } from "react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function Section({
  title,
  description,
  danger,
  card,
  headingId,
  children,
}: {
  title: string;
  description?: ReactNode;
  danger?: boolean;
  /** One card holding the heading, its description and the controls, ruled
   *  apart: a settings page's block, so its text sits with what it governs. */
  card?: boolean;
  /** Names the heading, which then takes the focus a control in the section
   *  hands it as it goes. */
  headingId?: string;
  children: ReactNode;
}) {
  const heading = (
    <div className={cn("grid gap-1", card && "py-gutter")}>
      <h2
        id={headingId}
        tabIndex={headingId ? -1 : undefined}
        className={cn("text-sm font-medium", danger && "text-destructive")}
      >
        {title}
      </h2>
      {description && <p className="text-sm text-muted-foreground">{description}</p>}
    </div>
  );
  if (card) {
    return (
      <section className="grid gap-gutter">
        <Card className={cn("gap-0 overflow-hidden py-0", danger && "bg-destructive/5 ring-destructive/40")}>
          {heading}
          <div
            className={cn(
              "grid gap-gutter border-t border-border py-gutter text-sm",
              danger && "border-destructive/20",
            )}
          >
            {children}
          </div>
        </Card>
      </section>
    );
  }
  return (
    <section className="grid gap-gutter">
      {heading}
      {children}
    </section>
  );
}
