"use client";

import { unstable_rethrow } from "next/navigation";
import { useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";

import { markProjectReadAction } from "./notice-actions";

export function MarkProjectRead({
  projectId,
  unread,
}: {
  /// The project this page rendered; the action marks no other.
  projectId: string;
  unread: number;
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
            const { error } = await markProjectReadAction(projectId);
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
