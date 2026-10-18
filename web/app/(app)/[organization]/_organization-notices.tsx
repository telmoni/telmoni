"use client";

import { unstable_rethrow } from "next/navigation";
import { useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";

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
  const [pending, start] = useTransition();

  return (
    <Button
      variant="outline"
      size="sm"
      className="h-9 text-xs"
      disabled={pending}
      onClick={() =>
        start(async () => {
          try {
            const { error } = await markOrganizationReadAction(organizationId, before);
            if (error) toast.error(error);
          } catch (err) {
            unstable_rethrow(err);
            toast.error("Something went wrong. Please try again.");
          }
        })
      }
    >
      {pending ? "Marking…" : `Mark read (${unread})`}
    </Button>
  );
}
