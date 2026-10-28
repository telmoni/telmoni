"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAction } from "@/lib/use-action";

import { forgotPasswordAction } from "../_accounts/actions";

export function ForgotForm() {
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);
  const { pending, error, run } = useAction();

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
        run(() => forgotPasswordAction({ email: email.trim() }), () => setSent(true));
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
