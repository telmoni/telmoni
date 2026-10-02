"use client";

import { useParams, useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
import { Button } from "@/components/ui/button";

import { revokeTokenAction, rotateTokenAction, type MintResult } from "./actions";

export function TokenRowActions({
  tokenId,
  name,
  canManage = true,
}: {
  tokenId: string;
  name: string;
  canManage?: boolean;
}) {
  const router = useRouter();
  const { projectId } = useParams<{ projectId: string }>();
  const [pending, startTransition] = useTransition();
  const [error, setError] = useState<string | null>(null);
  const [rotated, setRotated] = useState<MintResult | null>(null);

  if (!canManage) return null;

  function revoke() {
    startTransition(async () => {
      setError(null);
      try {
        const r = await revokeTokenAction(projectId, tokenId);
        if (r.error) setError(r.error);
        else router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  function rotate() {
    startTransition(async () => {
      setError(null);
      try {
        const r = await rotateTokenAction(projectId, tokenId);
        if (r.status === "error") setError(r.error ?? "Rotation failed");
        else {
          setRotated(r);
          router.refresh();
        }
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  return (
    <div className="flex items-center justify-end gap-3">
      {error && <span className="text-xs text-destructive">{error}</span>}

      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="gap-1.5 text-xs h-8 text-muted-foreground hover:text-foreground"
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
            className="gap-1.5 text-xs h-8 text-destructive hover:text-destructive hover:bg-destructive/10"
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

      {rotated?.status === "ok" && rotated.token && (
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
