"use client";

import Link from "next/link";
import { useState, useTransition } from "react";

import { Button, buttonVariants } from "@/components/ui/button";

import { verifyEmailAction } from "../_accounts/actions";

export function ConfirmEmail({ userId, token }: { userId: string; token: string }) {
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [pending, start] = useTransition();

  if (done) {
    return (
      <div className="grid gap-3 text-sm">
        <p>Your email address is confirmed. Sign in to get started.</p>
        <div>
          <Link href="/auth/login" className={buttonVariants()}>
            Sign in
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      <div>
        <Button
          disabled={pending}
          onClick={() =>
            start(async () => {
              setError(null);
              try {
                const res = await verifyEmailAction({ userId, token });
                if (res.error) setError(res.error);
                else setDone(true);
              } catch {
                setError("Network error. Try again.");
              }
            })
          }
        >
          {pending ? "Confirming…" : "Confirm"}
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
