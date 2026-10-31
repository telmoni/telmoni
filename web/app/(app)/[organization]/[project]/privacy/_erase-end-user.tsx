"use client";

import { useState } from "react";
import { toast } from "sonner";

import { eraseEndUserAction } from "@/app/(app)/[organization]/[project]/traces/actions";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useToastAction } from "@/lib/use-action";

/** A project admin's erasure of one end user, by the id the project's SDK
 *  sent as `user.id`: sent as typed, since ingest hashed it byte for byte. */
export function EraseEndUserForm({ projectId }: { projectId: string }) {
  const { pending, run } = useToastAction();
  const [userId, setUserId] = useState("");
  const [open, setOpen] = useState(false);

  const onConfirm = () =>
    run(
      () => eraseEndUserAction(projectId, userId),
      ({ traces }) => {
        setUserId("");
        toast.success(
          traces === 1 ? "Erased: 1 trace deleted." : `Erased: ${traces} traces deleted.`,
        );
      },
    );

  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
      <Input
        value={userId}
        onChange={(e) => setUserId(e.target.value)}
        disabled={pending}
        placeholder="The end user's id, as your app sends it"
        maxLength={256}
        className="max-w-md"
        aria-label="End user id"
        autoComplete="off"
      />
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        trigger={
          <Button variant="destructive" disabled={pending || userId.length === 0}>
            {pending ? "Erasing…" : "Erase"}
          </Button>
        }
        title="Erase this end user?"
        description="Every trace their id is on, the traces of their sessions that name no one, and any content those traces kept, are gone from every page at once. The audit log records the erasure by the id's keyed hash, and a receipt is mailed to you. There is no undo."
        confirmLabel="Erase end user"
        destructive
        onConfirm={onConfirm}
      />
    </div>
  );
}
