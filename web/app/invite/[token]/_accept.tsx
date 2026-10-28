"use client";

import { Button } from "@/components/ui/button";
import { useAction } from "@/lib/use-action";

import { acceptInviteAction } from "./actions";

export function AcceptInvite({ token }: { token: string }) {
  const { pending, error, run } = useAction();

  return (
    <div className="grid gap-2">
      <div>
        <Button
          disabled={pending}
          // An accept that landed goes on to the console in the action itself
          // (its `redirect`): what comes back here is a refusal.
          onClick={() => run(() => acceptInviteAction(token))}
        >
          {pending ? "Accepting…" : "Accept invitation"}
        </Button>
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
