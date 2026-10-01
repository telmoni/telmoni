"use client";

import { useRouter } from "next/navigation";
import { useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";

import { markOrganizationReadAction } from "./notice-actions";

export function MarkOrganizationRead({ unread }: { unread: number }) {
  const router = useRouter();
  const [pending, start] = useTransition();

  return (
    <Button
      variant="outline"
      size="sm"
      className="h-8 text-xs"
      disabled={pending}
      onClick={() =>
        start(async () => {
          try {
            const { error } = await markOrganizationReadAction();
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
