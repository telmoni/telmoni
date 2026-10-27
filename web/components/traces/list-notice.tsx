import type { ReactNode } from "react";

import { Card } from "@/components/ui/card";

/** What a list says in its table's place: nothing to show yet, nothing its
 *  filters admit, or a page past the last. */
export function ListNotice({ title, body }: { title: string; body: ReactNode }) {
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <div className="grid gap-1 p-4">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{body}</p>
      </div>
    </Card>
  );
}
