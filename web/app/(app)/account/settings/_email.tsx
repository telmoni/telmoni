"use client";

import { useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { emailPosture } from "@/lib/sign-in-method";

import {
  confirmEmailChangeAction,
  requestEmailChangeAction,
} from "./actions";

// The address is the visible layer over an account whose real base is its id,
// so moving it is a supported act rather than a rebuild. What makes it safe is
// that it takes two inboxes: the identity provider mails a code to the new
// address, and this platform mails one to the current address. The second is
// why a stolen session cookie is not enough — this is the lane that rewrites
// the address every other emailed gate sends to.
export function ChangeEmail({
  method,
  email,
}: {
  method: string | null;
  email: string;
}) {
  const [nextEmail, setNextEmail] = useState("");
  // ⚠ The address the SERVER echoed, not the one in the input. A boolean plus
  // the input would name the wrong destination the moment somebody edits the
  // field after sending, and would post an address the codes were not minted
  // for. The deletion flow needs none of this because its destination is the
  // account's own address, handed in as a prop.
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [currentCode, setCurrentCode] = useState("");
  const [newCode, setNewCode] = useState("");
  const [changedTo, setChangedTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Two transitions, so a spinning "Resend code" does not also disable the
  // confirm button's own label logic.
  const [sending, startSending] = useTransition();
  const [confirming, startConfirming] = useTransition();

  const posture = emailPosture(method);

  const trimmed = nextEmail.trim();
  const canSend =
    /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed) &&
    trimmed.toLowerCase() !== email.toLowerCase() &&
    !sending;
  const canConfirm =
    /^\d{6}$/.test(currentCode) && /^\d{6}$/.test(newCode) && !confirming;

  const send = () => {
    setError(null);
    startSending(async () => {
      try {
        const res = await requestEmailChangeAction(trimmed);
        if (res.error) {
          setError(res.error);
          return;
        }
        setSentTo(res.email ?? trimmed);
        setCurrentCode("");
        setNewCode("");
      } catch {
        setError("Network error. Try again.");
      }
    });
  };

  const confirm = () => {
    setError(null);
    startConfirming(async () => {
      try {
        const res = await confirmEmailChangeAction(currentCode, newCode);
        if (res.error) {
          setError(res.error);
          return;
        }
        // ⚠ **No refresh.** The session ended with the change, so a re-render
        // would redirect to sign-out mid-sentence, before the person has read
        // which address the account now has and why they are signed out.
        setChangedTo(res.email ?? sentTo);
      } catch {
        setError("Network error. Try again.");
      }
    });
  };

  // ⚠ **NOTHING, not an explanation.** `managed`, `provider` and `unknown`
  // each used to return a paragraph on why there was no form here — the
  // directory owns the address, Google would assert it back at the next
  // sign-in, the session does not say. The page no longer renders the Email
  // section for any of them at all, so this is unreachable from `page.tsx` and
  // exists to fail closed if some other caller ever mounts this: a form drawn
  // for an ineligible account would be refused by the Server Action anyway,
  // and showing one is a promise we cannot keep.
  if (posture !== "change") return null;

  if (changedTo) {
    return (
      <div className="grid gap-3">
        <p className="text-muted-foreground">
          Your address is now <span className="font-mono">{changedTo}</span>.
          Sign in with that from now on &mdash; your password has not changed.
        </p>
        {/* ⚠ **EVERY session, and that includes this browser.** Auth's
            `revoke_all_for_person` has no `keep` parameter, deliberately:
            signing in again with the new address is the only proof outside an
            inbox that the change took. So this cannot say "your OTHER devices"
            — auth already refuses this tab's bearer, and the action ended the
            console's session too, so the next press of anything lands on
            sign-in. Saying so here is the difference between a deliberate
            sign-out and a console that stops working right after a success. */}
        <p className="text-muted-foreground">
          Every signed-in session was ended, this one included &mdash; the
          address they were signed in with no longer exists. Signing in again
          with the new one is what confirms the change took.
        </p>
        <div>
          <Button
            onClick={() => {
              window.location.replace("/auth/logout");
            }}
          >
            Sign in again
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      {sentTo === null ? (
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSend) send();
          }}
        >
          <p className="text-muted-foreground">
            We send a 6-digit code to the new address and another to this one.
            Both are needed, so a mistyped address can&rsquo;t lock you out and
            nothing changes until you finish.
          </p>
          <div className="grid gap-2">
            <Label htmlFor="new-email">New email address</Label>
            <Input
              id="new-email"
              type="email"
              autoComplete="email"
              value={nextEmail}
              onChange={(e) => {
                setNextEmail(e.target.value);
                setError(null);
              }}
              placeholder="you@example.com"
              disabled={sending}
            />
          </div>
          <div>
            <Button type="submit" variant="outline" disabled={!canSend}>
              {sending ? "Sending…" : "Send confirmation codes"}
            </Button>
          </div>
        </form>
      ) : (
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (canConfirm) confirm();
          }}
        >
          <div className="grid gap-2">
            <Label htmlFor="email-code-new">
              Code sent to <span className="font-mono">{sentTo}</span>
            </Label>
            <Input
              id="email-code-new"
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={6}
              value={newCode}
              onChange={(e) => {
                setNewCode(e.target.value.replace(/\D/g, "").slice(0, 6));
                setError(null);
              }}
              placeholder="000000"
              disabled={confirming}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="email-code-current">
              Code sent to <span className="font-mono">{email}</span>
            </Label>
            <Input
              id="email-code-current"
              inputMode="numeric"
              maxLength={6}
              value={currentCode}
              onChange={(e) => {
                setCurrentCode(e.target.value.replace(/\D/g, "").slice(0, 6));
                setError(null);
              }}
              placeholder="000000"
              disabled={confirming}
            />
          </div>
          <div className="flex items-center gap-3">
            <Button type="submit" disabled={!canConfirm}>
              {confirming ? "Changing…" : "Change my email"}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={sending || confirming}
              onClick={send}
            >
              Resend codes
            </Button>
            {/* The deletion flow has no equivalent and this one needs it:
                without a way back, a typo'd address has no escape but a reload,
                which loses the state anyway. */}
            <Button
              type="button"
              variant="outline"
              disabled={sending || confirming}
              onClick={() => {
                setSentTo(null);
                setCurrentCode("");
                setNewCode("");
                setError(null);
              }}
            >
              Use a different address
            </Button>
          </div>
        </form>
      )}

      {/* Inline, not a toast. Every error here is about a field still on
          screen, and a toast that vanishes while somebody re-reads the field is
          the wrong affordance. The password control uses toasts because a
          one-press flow has nothing on screen to correct. */}
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
