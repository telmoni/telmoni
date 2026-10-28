import type { ComponentProps, ReactNode } from "react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

/** What a page says in a list's or a record's place: nothing to show yet,
 *  nothing its filters admit, a page past the last, or a read that did not
 *  land. */
export function NoticeCard({
  title,
  body,
  className,
  ...card
}: { title: string; body: ReactNode } & Omit<ComponentProps<typeof Card>, "title">) {
  return (
    <Card className={cn("gap-0 overflow-hidden p-0", className)} {...card}>
      <div className="grid gap-1 p-4">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{body}</p>
      </div>
    </Card>
  );
}
