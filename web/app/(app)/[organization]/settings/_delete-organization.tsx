"use client";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { CONFIRMATION_CODE_LENGTH } from "@/lib/confirmation-code";
import { useCodeConfirmed } from "@/lib/use-code-confirmed";

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
  const flow = useCodeConfirmed(
    () => requestOrganizationDeletionCodeAction(organizationId),
    async (code) => {
      const r = await deleteOrganizationAction(organizationId, code);
      // The name is kept with the outcome rather than read from props: anything
      // that refreshes the page from here on renders whichever organization the
      // console fell back to, and the heading must still name the deleted one.
      return r.ok ? { ...r, organization } : r;
    },
  );

  if (flow.step.step === "done") {
    const done = flow.step.done;
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
            <Label htmlFor="delete-organization-code">
              Enter the {CONFIRMATION_CODE_LENGTH}-digit code sent to{" "}
              <span className="font-medium">{email}</span>
            </Label>
            <Input
              id="delete-organization-code"
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
              {flow.confirming ? "Deleting…" : "Delete this organization"}
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
