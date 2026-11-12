"use client";

import { Button } from "@/components/ui/button";
import { useToastAction } from "@/lib/use-action";

import { unlinkSlackAction } from "./actions";

// One linked Slack account's Unlink, named for its workspace.
export function UnlinkSlack({ teamId, workspace }: { teamId: string; workspace: string }) {
  const { pending, run } = useToastAction();
  return (
    <Button
      variant="destructiveOutline"
      size="sm"
      disabled={pending}
      aria-label={`Unlink your Slack account in ${workspace}`}
      onClick={() => run(() => unlinkSlackAction(teamId), `Unlinked from ${workspace}.`)}
    >
      {pending ? "Unlinking…" : "Unlink"}
    </Button>
  );
}
