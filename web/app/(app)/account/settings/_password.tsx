"use client";

import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { passwordPosture } from "@/lib/sign-in-method";

import { requestPasswordResetAction } from "./actions";

export function PasswordReset({ method }: { method: string | null }) {
  const [pending, startTransition] = useTransition();
  const [sent, setSent] = useState(false);

  const posture = passwordPosture(method);

  const onPress = () => {
    startTransition(async () => {
      try {
        const res = await requestPasswordResetAction();
        if (res.error) {
          toast.error(res.error);
          return;
        }
        setSent(true);
        toast.success("Check your email for a link to set a new password.");
      } catch {
        toast.error("Network error. Please try again.");
      }
    });
  };

  // ⚠ **NOTHING, not an explanation** — and still no offer to "add" a password
  // for a provider account; see `passwordPosture` for why the second way in was
  // the same way in. `managed`, `provider` and `unknown` each used to return a
  // paragraph here. The page no longer renders the Password section for any of
  // them, so this is unreachable from `page.tsx` and is the fail-closed guard
  // for any other caller: drawing the button would offer to mint a credential
  // on an account whose provider already holds one.
  if (posture !== "reset") return null;

  return (
    <div className="grid gap-3">
      <p className="text-muted-foreground">
        The link works once and expires within the hour. Every signed-in
        session ends when you use it, this one included.
      </p>
      <div>
        <Button variant="outline" onClick={onPress} disabled={pending || sent}>
          {pending
            ? "Sending…"
            : sent
              ? "Link sent"
              : "Email me a password reset link"}
        </Button>
      </div>
    </div>
  );
}
