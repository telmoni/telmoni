"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";

import { leaveProjectAction } from "../[projectId]/members/actions";

export function LeaveProject({
  projectId,
  name,
}: {
  projectId: string;
  name: string;
}) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  return (
    <div className="flex items-center justify-end gap-3">
      {error && <span className="text-xs text-destructive">{error}</span>}
      <ConfirmDialog
        title="Leave project"
        description={
          <>
            Leave <span className="font-medium text-foreground">{name}</span>?
            You will lose access to its API keys and settings immediately, and
            only an admin can invite you back.
          </>
        }
        confirmLabel="Leave project"
        destructive
        onConfirm={() => {
          setError(null);
          start(async () => {
            const res = await leaveProjectAction(projectId);
            if (res?.error) setError(res.error);
            else router.refresh();
          });
        }}
        trigger={
          <Button
            size="sm"
            variant="ghost"
            disabled={pending}
            data-testid={`leave-project-${projectId}`}
            className="text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            Leave
          </Button>
        }
      />
    </div>
  );
}
