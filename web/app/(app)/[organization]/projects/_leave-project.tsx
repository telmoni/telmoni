"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";

import { leaveProjectAction } from "../[project]/members/actions";

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
        onConfirm={() =>
          start(async () => {
            setError(null);
            try {
              const r = await leaveProjectAction(projectId);
              if (r.error) setError(r.error);
              else router.refresh();
            } catch {
              setError("Network error. Try again.");
            }
          })
        }
      />
    </div>
  );
}
