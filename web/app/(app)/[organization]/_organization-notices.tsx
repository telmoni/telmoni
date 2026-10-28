"use client";

import { Button } from "@/components/ui/button";
import { useToastAction } from "@/lib/use-action";

import { markOrganizationReadAction } from "./notice-actions";

export function MarkOrganizationRead({
  organizationId,
  unread,
  before,
}: {
  /// The organization this page rendered; the action refuses any other.
  organizationId: string;
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
      onClick={() => run(() => markOrganizationReadAction(organizationId, before))}
    >
      {pending ? "Marking…" : `Mark read (${unread})`}
    </Button>
  );
}
