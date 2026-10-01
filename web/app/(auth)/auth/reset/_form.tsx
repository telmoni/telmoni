"use client";

import Link from "next/link";
import { useState, useTransition } from "react";

import { PasswordInput } from "@/components/password-input";
import { Button, buttonVariants } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

import { resetPasswordAction } from "../_accounts/actions";

export function ResetForm({ userId, token }: { userId: string; token: string }) {
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [pending, start] = useTransition();

  if (done) {
    return (
      <div className="grid gap-3 text-sm">
        <p>Your password is changed. Sign in with it from now on.</p>
        <div>
          <Link href="/auth/login" className={buttonVariants()}>
            Sign in
          </Link>
        </div>
      </div>
    );
  }

  const mismatch = again.length > 0 && again !== password;

  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (pending || mismatch) return;
        setError(null);
        start(async () => {
          try {
            const res = await resetPasswordAction({ userId, token, password });
            if (res.error) setError(res.error);
            else setDone(true);
          } catch {
            setError("Network error. Try again.");
          }
        });
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="password">New password</Label>
        <PasswordInput
          id="password"
          name="password"
          autoComplete="new-password"
          required
          minLength={8}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <p className="text-xs text-muted-foreground">At least 8 characters.</p>
      </div>
      <div className="grid gap-2">
        <Label htmlFor="again">New password, again</Label>
        <PasswordInput
          id="again"
          name="again"
          autoComplete="new-password"
          required
          aria-invalid={mismatch || undefined}
          value={again}
          onChange={(e) => setAgain(e.target.value)}
        />
        {mismatch && <p className="text-xs text-destructive">The two do not match.</p>}
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <Button type="submit" disabled={pending || password.length < 8 || again !== password}>
        {pending ? "Saving…" : "Change password"}
      </Button>
    </form>
  );
}
