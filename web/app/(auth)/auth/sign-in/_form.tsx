"use client";

import { useState } from "react";

import { PasswordInput } from "@/components/password-input";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAction } from "@/lib/use-action";

import { signInAction } from "../_accounts/actions";

// The form, and beside it the provider's button where the deployment has
// one; an address whose domain is held to the provider sees the button
// alone.
export function SignInForm({
  state,
  email: initialEmail,
  provider,
}: {
  state: string;
  email: string;
  provider: string | null;
}) {
  const [email, setEmail] = useState(initialEmail);
  const [password, setPassword] = useState("");
  const [sso, setSso] = useState(false);
  const { pending, error, run } = useAction();

  const submit = () => {
    run(
      async () => {
        const res = await signInAction({ email: email.trim(), password, state });
        if (!res.ok && "sso" in res) setSso(true);
        return res;
      },
      // The callback, with the code and the state the door sealed: the same
      // hop an external provider's page makes, from here.
      (res) => window.location.replace(res.next),
    );
  };

  // A Route Handler, not a page: it carries the pending sign-in on to the
  // provider, so a client-side navigation would lose it.
  const continueWith = provider && (
    // eslint-disable-next-line @next/next/no-html-link-for-pages
    <a
      href="/auth/login/external"
      className={buttonVariants({ variant: sso ? "default" : "outline" })}
    >
      Continue with {provider}
    </a>
  );

  if (sso && continueWith) {
    return (
      <div className="grid gap-3">
        <p className="text-sm text-muted-foreground" role="status">
          {error}
        </p>
        {continueWith}
      </div>
    );
  }

  return (
    <div className="grid gap-3">
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
      {continueWith && (
        <>
          <p className="text-center text-xs uppercase text-muted-foreground">or</p>
          {continueWith}
        </>
      )}
    </div>
  );
}
