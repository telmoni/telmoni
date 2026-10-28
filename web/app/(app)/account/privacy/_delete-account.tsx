"use client";

import { useEffect } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { signOut } from "@/lib/auth/sign-out";
import { CONFIRMATION_CODE_LENGTH } from "@/lib/confirmation-code";
import { PRODUCT_NAME } from "@/lib/site";
import { useCodeConfirmed } from "@/lib/use-code-confirmed";

import { deleteAccountAction, requestAccountDeletionCodeAction } from "./actions";

const FAREWELL_MS = 6000;

/// Deleting the ACCOUNT: the person's sign-in, and every organization they
/// own on their own. An organization they own with anybody else in it blocks
/// it — auth answers with their names, and this form shows that answer as the
/// next step to take.
export function DeleteAccountForm({
  email,
  ownedOrganizations,
}: {
  email: string;
  /// How many organizations this person owns: they go with the account when
  /// nobody else is in them, and block it when somebody is. Counted, not
  /// named: twenty names read as a list, and the switcher has them all.
  ownedOrganizations: number;
}) {
  const flow = useCodeConfirmed(requestAccountDeletionCodeAction, deleteAccountAction);
  const farewell = flow.step.step === "done";

  useEffect(() => {
    if (!farewell) return;
    const t = setTimeout(signOut, FAREWELL_MS);
    return () => clearTimeout(t);
  }, [farewell]);

  if (flow.step.step === "done") {
    return (
      <div className="grid gap-4">
        <h3 className="text-base font-medium">Sorry to see you go</h3>
        <p className="text-sm text-muted-foreground">
          {flow.step.done.deleted === true
            ? "Your account has been deleted: your sign-in is gone, and so is every organization you owned on your own. "
            : "Your deletion is confirmed and underway. Your access ends now, and your sign-in and the organizations you owned on your own are erased shortly, usually within the hour. "}
          Thank you for having used {PRODUCT_NAME} — if you ever want to come
          back, you&rsquo;re always welcome.
        </p>
        <p className="text-sm text-muted-foreground">
          Signing you out and taking you to the home page&hellip;
        </p>
        <div>
          <Button variant="destructive" onClick={signOut}>
            Sign out now
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-4">
      <p className="text-sm text-muted-foreground">
        This is permanent, with no undo: your sign-in is erased, you leave
        every organization you are in, and they keep everything. To close one
        organization and keep your account, delete it from its own settings
        instead.
      </p>
      {ownedOrganizations > 0 && (
        <div className="grid gap-1">
          <p className="text-sm">
            You own{" "}
            <span className="font-medium">
              {ownedOrganizations} {ownedOrganizations === 1 ? "organization" : "organizations"}
            </span>
            .
          </p>
          <p className="text-sm text-muted-foreground">
            An organization you own on your own is deleted with your account,
            with its projects and API keys. One that anybody else
            is in has to be handed to one of its admins, or emptied, first —
            deleting your account will say which.
          </p>
        </div>
      )}

      {flow.step.step === "ask" ? (
        <div>
          <Button variant="destructive" disabled={flow.sending} onClick={flow.sendCode}>
            {flow.sending ? "Sending…" : "Send confirmation code"}
          </Button>
        </div>
      ) : (
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (flow.canConfirm) flow.confirmCode();
          }}
        >
          <div className="grid gap-2">
            <Label htmlFor="delete-account-code">
              Enter the {CONFIRMATION_CODE_LENGTH}-digit code sent to{" "}
              <span className="font-medium">{email}</span>
            </Label>
            <Input
              id="delete-account-code"
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={CONFIRMATION_CODE_LENGTH}
              value={flow.code}
              onChange={(e) => flow.type(e.target.value)}
              placeholder="000000"
              disabled={flow.confirming}
            />
          </div>
          <div className="flex items-center gap-3">
            <Button type="submit" variant="destructive" disabled={!flow.canConfirm}>
              {flow.confirming ? "Deleting…" : "Permanently delete my account"}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={flow.sending || flow.confirming}
              onClick={flow.sendCode}
            >
              Resend code
            </Button>
          </div>
        </form>
      )}

      {flow.error && (
        <p className="text-sm text-destructive" role="alert">
          {flow.error}
        </p>
      )}
    </div>
  );
}
