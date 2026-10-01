"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";

import { deleteProjectAction } from "./actions";

export function DeleteProjectForm({
  projectId,
  projectName,
}: {
  projectId: string;
  projectName: string;
}) {
  const router = useRouter();
  const [pending, startTransition] = useTransition();
  const [open, setOpen] = useState(false);

  const onConfirm = () => {
    startTransition(async () => {
      try {
        const res = await deleteProjectAction(projectId);
        if (res.error) {
          toast.error(res.error);
          return;
        }
        toast.success(`${projectName} deleted.`);
        // No `refresh()` beside it: it can re-render the route still showing,
        // the deleted project's own page, before the move lands. The action's
        // revalidation already drops the stale rail. `replace`, so Back does
        // not return to a project that is gone.
        router.replace("/console");
      } catch {
        toast.error("Network error. Please try again.");
      }
    });
  };

  return (
    <div className="grid gap-3">
      <p className="text-muted-foreground">
        Deleting <span className="font-medium text-foreground">{projectName}</span>{" "}
        removes its API keys, members and pending invitations. Keys stop
        working immediately — anything deploying with one will start failing.
        This cannot be undone.
      </p>
      <div>
        <ConfirmDialog
          open={open}
          onOpenChange={setOpen}
          trigger={
            <Button variant="destructive" disabled={pending}>
              {pending ? "Deleting…" : "Delete project"}
            </Button>
          }
          title={`Delete ${projectName}?`}
          description="Its keys, destinations, members and invitations go with it. There is no undo and no grace period."
          confirmLabel="Delete project"
          destructive
          confirmPhrase={projectName}
          onConfirm={onConfirm}
        />
      </div>
    </div>
  );
}
