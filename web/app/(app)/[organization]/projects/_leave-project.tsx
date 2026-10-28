"use client";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { useAction } from "@/lib/use-action";

import { leaveProjectAction } from "../[project]/members/actions";

export function LeaveProject({
  projectId,
  name,
}: {
  projectId: string;
  name: string;
}) {
  const { pending, error, run } = useAction();

  return (
    <div className="flex items-center justify-end gap-3">
      {error && <span className="text-xs text-destructive">{error}</span>}
      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-xs"
            disabled={pending}
            aria-label={`Leave ${name}`}
          >
            Leave
          </Button>
        }
        title={`Leave ${name}?`}
        description="You will lose access to this project immediately."
        confirmLabel="Leave"
        onConfirm={() => run(() => leaveProjectAction(projectId))}
      />
    </div>
  );
}
