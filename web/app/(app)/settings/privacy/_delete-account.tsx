"use client";

import { useEffect } from "react";

import { CodeField, CodeSentTo, ResendCodeButton } from "@/components/code-field";
import { Button } from "@/components/ui/button";
import { signOut } from "@/lib/auth/sign-out";
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
      <div className="grid gap-gutter">
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
          <Button variant="filled" onClick={signOut}>
            Sign out now
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-gutter">
      <p className="text-sm text-muted-foreground">
        Deleting your account is permanent and can&rsquo;t be undone. Your
        sign-in is erased and you leave every organization you belong to;
        anything you added stays with them. To close a single organization
        but keep your account, delete that organization from its settings
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
            {ownedOrganizations === 1
              ? "If you\u2019re its only member, it\u2019s deleted along with your account, including its projects and API keys. If anyone else belongs to it, you\u2019ll first need to hand it to one of its admins or remove its other members."
              : "Any you\u2019re the only member of is deleted along with your account, including its projects and API keys. Any that someone else belongs to must first be handed to one of its admins, or have its other members removed."}{" "}
            You&rsquo;ll be told which applies when you delete your account.
          </p>
        </div>
      )}

      {flow.step.step === "ask" ? (
        <div>
          <Button variant="destructiveOutline" disabled={flow.sending} onClick={flow.sendCode}>
            {flow.sending ? "Sending…" : "Send confirmation code"}
          </Button>
        </div>
      ) : (
        <form
          className="grid gap-gutter"
          onSubmit={(e) => {
            e.preventDefault();
            if (flow.canConfirm) flow.confirmCode();
          }}
        >
          <CodeField
            id="delete-account-code"
            label={<CodeSentTo email={email} />}
            value={flow.code}
            onType={flow.type}
            disabled={flow.sending || flow.confirming}
          />
          <div className="flex items-center gap-gutter">
            <Button type="submit" variant="destructive" disabled={!flow.canConfirm}>
              {flow.confirming ? "Deleting…" : "Permanently delete my account"}
            </Button>
            <ResendCodeButton sending={flow.sending} confirming={flow.confirming} resendIn={flow.resendIn} onResend={flow.sendCode} />
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
