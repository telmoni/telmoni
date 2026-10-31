"use client";

import { useState } from "react";

import { deleteTraceAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { useToastAction } from "@/lib/use-action";

/** A project admin's Delete on a trace's page: confirmed, then the list,
 *  where the trace no longer shows. */
export function DeleteTrace({ projectId, runId }: { projectId: string; runId: string }) {
  const { pending, run } = useToastAction();
  const [open, setOpen] = useState(false);

  // A deletion that landed leaves for the list in the action itself (its
  // `redirect`): what comes back here is a refusal.
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={setOpen}
      trigger={
        <Button variant="destructive" disabled={pending}>
          {pending ? "Deleting…" : "Delete"}
        </Button>
      }
      title="Delete this trace?"
      description="Its spans and any content they kept are gone from every page at once. The deletion is recorded in the audit log, and a receipt is mailed to you. There is no undo."
      confirmLabel="Delete trace"
      destructive
      onConfirm={() => run(() => deleteTraceAction(projectId, runId))}
    />
  );
}
