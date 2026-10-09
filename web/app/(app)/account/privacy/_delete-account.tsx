"use client";

import { useEffect, useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PRODUCT_NAME } from "@/lib/site";

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
  const [codeSent, setCodeSent] = useState(false);
  const [code, setCode] = useState("");
  const [farewell, setFarewell] = useState(false);
  const [erased, setErased] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sending, startSending] = useTransition();
  const [deleting, startDeleting] = useTransition();

  useEffect(() => {
    if (!farewell) return;
    const t = setTimeout(() => window.location.replace("/auth/logout"), FAREWELL_MS);
    return () => clearTimeout(t);
  }, [farewell]);

  function handleSendCode() {
    setError(null);
    startSending(async () => {
      try {
        const r = await requestAccountDeletionCodeAction();
        if (r.error) {
          setError(r.error);
          return;
        }
        setCodeSent(true);
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  function handleDelete() {
    setError(null);
    startDeleting(async () => {
      try {
        const r = await deleteAccountAction(code);
        if (r.error) {
          setError(r.error);
          return;
        }
        setErased(r.deleted === true);
        setFarewell(true);
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  const canDelete = /^\d{6}$/.test(code) && !deleting;

  if (farewell) {
    return (
      <div className="grid gap-4">
        <h3 className="text-base font-medium">Sorry to see you go</h3>
        <p className="text-sm text-muted-foreground">
          {erased
            ? "Your account has been deleted: your sign-in is gone, and so is every organization you owned on your own. "
            : "Your deletion is confirmed and underway. Your access ends now, and your sign-in and the organizations you owned on your own are erased shortly, usually within the hour. "}
          Thank you for having used {PRODUCT_NAME} — if you ever want to come
          back, you&rsquo;re always welcome.
        </p>
        <p className="text-sm text-muted-foreground">
          Signing you out and taking you to the home page&hellip;
        </p>
        <div>
          <Button
            variant="destructive"
            onClick={() => window.location.replace("/auth/logout")}
          >
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

      {!codeSent ? (
        <div>
          <Button variant="destructive" disabled={sending} onClick={handleSendCode}>
            {sending ? "Sending…" : "Send confirmation code"}
          </Button>
        </div>
      ) : (
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (canDelete) handleDelete();
          }}
        >
          <div className="grid gap-2">
            <Label htmlFor="delete-account-code">
              Enter the 6-digit code sent to{" "}
              <span className="font-mono">{email}</span>
            </Label>
            <Input
              id="delete-account-code"
              inputMode="numeric"
              autoComplete="one-time-code"
              maxLength={6}
              value={code}
              onChange={(e) => {
                setCode(e.target.value.replace(/\D/g, "").slice(0, 6));
                setError(null);
              }}
              placeholder="000000"
              disabled={deleting}
            />
          </div>
          <div className="flex items-center gap-3">
            <Button type="submit" variant="destructive" disabled={!canDelete}>
              {deleting ? "Deleting…" : "Permanently delete my account"}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={sending || deleting}
              onClick={handleSendCode}
            >
              Resend code
            </Button>
          </div>
        </form>
      )}

      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
