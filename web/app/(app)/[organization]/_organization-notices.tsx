"use client";

import { useRouter } from "next/navigation";
import { useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";

import { markOrganizationReadAction } from "./notice-actions";

export function MarkOrganizationRead({
  organizationId,
  unread,
}: {
  /// The organization this page rendered; the action refuses any other.
  organizationId: string;
  unread: number;
}) {
  const router = useRouter();
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
            const { error } = await markOrganizationReadAction(organizationId);
            if (error) {
              toast.error(error);
              return;
            }
            router.refresh();
          } catch {
            toast.error("Something went wrong. Please try again.");
          }
        })
      }
    >
      {pending ? "Marking…" : `Mark read (${unread})`}
    </Button>
  );
}
