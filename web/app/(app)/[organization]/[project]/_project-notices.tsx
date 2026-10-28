"use client";

import { Button } from "@/components/ui/button";
import { useToastAction } from "@/lib/use-action";

import { markProjectReadAction } from "./notice-actions";

export function MarkProjectRead({
  projectId,
  unread,
  before,
}: {
  /// The project this page rendered; the action marks no other.
  projectId: string;
  unread: number;
  /// The newest notice drawn, by its `created_at`; nothing newer is marked.
  before: string;
}) {
  const { pending, run } = useToastAction();

  return (
    <Button
      variant="outline"
      size="sm"
      className="h-9 text-xs"
      disabled={pending}
      onClick={() => run(() => markProjectReadAction(projectId, before))}
    >
      {pending ? "Marking…" : `Mark read (${unread})`}
    </Button>
  );
}
