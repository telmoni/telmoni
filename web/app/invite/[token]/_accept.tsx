"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

import { Button } from "@/components/ui/button";

import { acceptInviteAction } from "./actions";

export function AcceptInvite({ token }: { token: string }) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  return (
    <div className="grid gap-2">
      <div>
        <Button
          disabled={pending}
          onClick={() =>
            start(async () => {
              setError(null);
              try {
                const r = await acceptInviteAction(token);
                if (r.error) setError(r.error);
                else router.push("/console");
              } catch {
                setError("Network error. Try again.");
              }
            })
          }
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
