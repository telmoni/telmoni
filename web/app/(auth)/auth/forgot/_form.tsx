"use client";

import { useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import { forgotPasswordAction } from "../_accounts/actions";

export function ForgotForm() {
  const [email, setEmail] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [pending, start] = useTransition();

  if (sent) {
    return (
      <p className="text-sm">
        If an account holds that address, a link is on its way. It works once and expires in 60
        minutes.
      </p>
    );
  }

  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (pending) return;
        setError(null);
        start(async () => {
          try {
            const res = await forgotPasswordAction({ email: email.trim() });
            if (res.error) setError(res.error);
            else setSent(true);
          } catch {
            setError("Network error. Try again.");
          }
        });
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="email">Email address</Label>
        <Input
          id="email"
          name="email"
          type="email"
          autoComplete="email"
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
        />
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <Button type="submit" disabled={pending || !email.trim()}>
        {pending ? "Sending…" : "Email me a link"}
      </Button>
    </form>
  );
}
