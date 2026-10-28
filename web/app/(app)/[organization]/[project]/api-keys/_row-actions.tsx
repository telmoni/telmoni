"use client";

import { startTransition, useState } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
import { Button } from "@/components/ui/button";
import { useAction } from "@/lib/use-action";

import { revokeTokenAction, rotateTokenAction, type MintedKey } from "./actions";

export function TokenRowActions({
  projectId,
  tokenId,
  name,
}: {
  /// The project this page rendered; the actions act on it or on nothing.
  projectId: string;
  tokenId: string;
  name: string;
}) {
  const { pending, error, run } = useAction();
  const [rotated, setRotated] = useState<MintedKey | null>(null);


  function revoke() {
    run(() => revokeTokenAction(projectId, tokenId));
  }

  function rotate() {
    run(
      () => rotateTokenAction(projectId, tokenId),
      // In the transition, with the list the action drew again: the new key's
      // dialog opens as the list shows it.
      (r) => startTransition(() => setRotated(r)),
    );
  }

  return (
    <div className="flex items-center justify-end gap-3">
      {error && <span className="text-xs text-destructive">{error}</span>}

      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="gap-1.5 text-xs h-9 text-muted-foreground hover:text-foreground"
            disabled={pending}
            aria-label={`Rotate ${name}`}
          >
            Rotate
          </Button>
        }
        title={`Rotate "${name}"?`}
        description="The old key stays valid for a 24h grace window, then is invalidated."
        confirmLabel="Rotate"
        onConfirm={rotate}
      />
      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="gap-1.5 text-xs h-9 text-destructive hover:text-destructive hover:bg-destructive/10"
            disabled={pending}
            aria-label={`Revoke ${name}`}
          >
            Revoke
          </Button>
        }
        title={`Revoke "${name}"?`}
        description="This immediately and permanently invalidates the key."
        confirmLabel="Revoke"
        destructive
        onConfirm={revoke}
      />

      {rotated && (
        <PlaintextTokenDialog
          token={rotated.token}
          title="Rotated key"
          description="The previous key works for ~24h, then is revoked. Copy the new value now."
          closeLabel="Close"
          onClose={() => setRotated(null)}
        />
      )}
    </div>
  );
}
