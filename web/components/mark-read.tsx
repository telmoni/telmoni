"use client";

import { Button } from "@/components/ui/button";
import type { ActionResult } from "@/lib/action-result";
import { useToastAction } from "@/lib/use-action";

/** A feed marked read up to the newest notice drawn: `mark` is the page's
 *  action, bound to the organization or project it rendered and to that
 *  notice, so nothing newer is marked and no other feed. */
export function MarkRead({ unread, mark }: { unread: number; mark: () => Promise<ActionResult> }) {
  const { pending, run } = useToastAction();

  return (
    <Button
      variant="outline"
      size="sm"
      className="h-9 text-xs"
      disabled={pending}
      onClick={() => run(mark)}
    >
      {pending ? "Marking…" : `Mark read (${unread})`}
    </Button>
  );
}
