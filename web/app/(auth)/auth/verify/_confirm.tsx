"use client";

import { useState } from "react";

import { Button, buttonVariants } from "@/components/ui/button";
import { useAction } from "@/lib/use-action";

import { verifyEmailAction } from "../_accounts/actions";

export function ConfirmEmail({ userId, token }: { userId: string; token: string }) {
  const [done, setDone] = useState(false);
  const { pending, error, run } = useAction();

  if (done) {
    return (
      <div className="grid gap-3 text-sm">
        <p>Your email address is confirmed. Sign in to get started.</p>
        <div>
          {/* A Route Handler, not a page, so never a <Link>: a link's prefetch
              would run it, starting a sign-in nobody asked for. */}
          {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
          <a href="/auth/login" className={buttonVariants()}>
            Sign in
          </a>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      <div>
        <Button
          disabled={pending}
          onClick={() => run(() => verifyEmailAction({ userId, token }), () => setDone(true))}
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
