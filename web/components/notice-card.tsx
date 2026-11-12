import type { ComponentProps, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

/** What a page says in a list's or a record's place: nothing to show yet,
 *  nothing its filters admit, a page past the last, or a read that did not
 *  land. An empty list passes its page's rail icon. */
export function NoticeCard({
  title,
  body,
  icon: Icon,
  className,
  ...card
}: { title: string; body: ReactNode; icon?: LucideIcon } & Omit<ComponentProps<typeof Card>, "title">) {
  return (
    <Card className={cn("gap-0 overflow-hidden p-0", className)} {...card}>
      <div className="flex items-start gap-gutter p-gutter">
        {Icon && (
          <span className="flex size-control shrink-0 items-center justify-center rounded-full bg-action/10 text-action">
            <Icon className="size-icon" aria-hidden="true" />
          </span>
        )}
        <div className="grid min-w-0 gap-1">
          <p className="text-sm font-medium">{title}</p>
          <p className="text-sm text-muted-foreground">{body}</p>
        </div>
      </div>
    </Card>
  );
}
