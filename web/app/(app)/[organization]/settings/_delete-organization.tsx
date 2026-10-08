"use client";

import { useState, useTransition } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import {
  deleteOrganizationAction,
  requestOrganizationDeletionCodeAction,
} from "./deletion-actions";

/// Deleting the organization the page rendered. The owner keeps their
/// account: once it is closed the console takes them to wherever they still
/// belong, rather than signing them out as deleting an account does.
export function DeleteOrganizationForm({
  organizationId,
  organization,
  email,
}: {
  /// The organization this page rendered; the actions refuse to act on any
  /// other, whichever one their request resolves once this page has gone stale.
  organizationId: string;
  organization: string;
  email: string;
}) {
  const [codeSent, setCodeSent] = useState(false);
  const [code, setCode] = useState("");
  // The name is kept with the outcome rather than read from props: anything
  // that refreshes the page from here on renders whichever organization the
  // console fell back to, and the heading must still name the deleted one.
  const [done, setDone] = useState<{ organization: string; purged?: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sending, startSending] = useTransition();
  const [deleting, startDeleting] = useTransition();

  function handleSendCode() {
    setError(null);
    startSending(async () => {
      try {
        const r = await requestOrganizationDeletionCodeAction(organizationId);
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
        const r = await deleteOrganizationAction(organizationId, code);
        if (r.error) {
          setError(r.error);
          return;
        }
        setDone({ organization, purged: r.purged });
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  const canDelete = /^\d{6}$/.test(code) && !deleting;

  if (done) {
    return (
      <div className="grid gap-4">
        <h3 className="text-base font-medium">{done.organization} is deleted</h3>
        <p className="text-sm text-muted-foreground">
          Everyone in it has lost access and its API keys have stopped working.
          {done.purged === false &&
            " Part of its cleanup could not finish just now; Telmoni retries every ten minutes until it does."}{" "}
          Its projects and everything else it held are erased from our systems
          within the hour. Your own account is untouched.
        </p>
        <div>
          <Button onClick={() => window.location.replace("/console")}>
            Continue
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-4">
      <p className="text-sm text-muted-foreground">
        Everyone in it loses access and its API keys stop working the moment
        you confirm, and its projects and data are erased for good within the
        hour. There is no undo. Nobody&rsquo;s account is deleted, yours
        included.
      </p>

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
            <Label htmlFor="delete-organization-code">
              Enter the 6-digit code sent to{" "}
              <span className="font-mono">{email}</span>
            </Label>
            <Input
              id="delete-organization-code"
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
              {deleting ? "Deleting…" : "Delete this organization"}
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
