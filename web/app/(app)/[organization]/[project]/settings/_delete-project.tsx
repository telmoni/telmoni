"use client";

import { useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { useToastAction } from "@/lib/use-action";

import { deleteProjectAction } from "./actions";

export function DeleteProjectForm({
  projectId,
  projectName,
}: {
  projectId: string;
  projectName: string;
}) {
  const { pending, run } = useToastAction();
  const [open, setOpen] = useState(false);

  // A deletion that landed leaves for the console in the action itself (its
  // `redirect`): what comes back here is a refusal.
  const onConfirm = () => run(() => deleteProjectAction(projectId));

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
