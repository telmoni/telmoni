import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

export function Section({
  title,
  description,
  danger,
  headingId,
  children,
}: {
  title: string;
  description?: ReactNode;
  danger?: boolean;
  /** Names the heading, which then takes the focus a control in the section
   *  hands it as it goes. */
  headingId?: string;
  children: ReactNode;
}) {
  return (
    <section className="grid gap-3">
      <div className="grid gap-1">
        <h2
          id={headingId}
          tabIndex={headingId ? -1 : undefined}
          className={cn("text-sm font-medium", danger && "text-destructive")}
        >
          {title}
        </h2>
        {description && (
          <p className="text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}
