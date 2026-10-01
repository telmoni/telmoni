"use client";

import { useState, useTransition } from "react";

import { PasswordInput } from "@/components/password-input";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import { signUpAction } from "../_accounts/actions";

export function SignUpForm({
  state,
  email: initialEmail,
  verifyEmail,
}: {
  state: string;
  email: string;
  verifyEmail: boolean;
}) {
  const [givenName, setGivenName] = useState("");
  const [familyName, setFamilyName] = useState("");
  const [email, setEmail] = useState(initialEmail);
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  // The address the account was created under, once it was and a mail is
  // what comes next.
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const submit = () => {
    setError(null);
    start(async () => {
      try {
        const res = await signUpAction({
          email: email.trim(),
          password,
          givenName: givenName.trim(),
          familyName: familyName.trim(),
          state,
        });
        if (res.error) {
          setError(res.error);
          return;
        }
        // With no address to confirm, the account is signed in at once:
        // the callback, with the code and the state the door sealed.
        if (res.next) {
          window.location.replace(res.next);
          return;
        }
        setSentTo(email.trim());
      } catch {
        setError("Network error. Try again.");
      }
    });
  };

  if (sentTo) {
    return (
      <div className="grid gap-3 text-sm">
        <p>
          Check your email. We sent a link to <span className="font-mono">{sentTo}</span> to
          confirm it is yours; it works once and expires in 24 hours.
        </p>
        <p className="text-muted-foreground">
          Nothing arrived? Look in spam, then sign in with your password to have another sent.
        </p>
      </div>
    );
  }

  const mismatch = again.length > 0 && again !== password;

  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!pending && !mismatch) submit();
      }}
    >
      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-2">
          <Label htmlFor="given-name">First name</Label>
          <Input
            id="given-name"
            name="given-name"
            autoComplete="given-name"
            value={givenName}
            onChange={(e) => setGivenName(e.target.value)}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="family-name">Last name</Label>
          <Input
            id="family-name"
            name="family-name"
            autoComplete="family-name"
            value={familyName}
            onChange={(e) => setFamilyName(e.target.value)}
          />
        </div>
      </div>
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
          autoComplete="new-password"
          required
          minLength={8}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <p className="text-xs text-muted-foreground">At least 8 characters.</p>
      </div>
      <div className="grid gap-2">
        <Label htmlFor="again">Confirm password</Label>
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
      <Button type="submit" disabled={pending || !email.trim() || password.length < 8 || again !== password}>
        {pending ? "Creating…" : verifyEmail ? "Create account" : "Create account and sign in"}
      </Button>
    </form>
  );
}
