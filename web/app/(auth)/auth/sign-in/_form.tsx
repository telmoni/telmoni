"use client";

import { useState, useTransition } from "react";

import { PasswordInput } from "@/components/password-input";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import { signInAction } from "../_accounts/actions";

export function SignInForm({ state, email: initialEmail }: { state: string; email: string }) {
  const [email, setEmail] = useState(initialEmail);
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const submit = () => {
    setError(null);
    start(async () => {
      try {
        const res = await signInAction({ email: email.trim(), password, state });
        if (res.error) {
          setError(res.error);
          return;
        }
        // The callback, with the code and the state the door sealed: the
        // same hop an external provider's page makes, from here.
        if (res.next) window.location.replace(res.next);
      } catch {
        setError("Network error. Try again.");
      }
    });
  };

  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!pending) submit();
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
      <div className="grid gap-2">
        <Label htmlFor="password">Password</Label>
        <PasswordInput
          id="password"
          name="password"
          autoComplete="current-password"
          required
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <Button type="submit" disabled={pending || !email.trim() || !password}>
        {pending ? "Signing in…" : "Sign in"}
      </Button>
    </form>
  );
}
