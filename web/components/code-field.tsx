import type { ReactNode } from "react";

import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { CONFIRMATION_CODE_LENGTH } from "@/lib/confirmation-code";

/** The field an emailed confirmation code is typed into, under its label. */
export function CodeField({
  id,
  label,
  value,
  onType,
  disabled,
  autoFill = true,
}: {
  id: string;
  label: ReactNode;
  value: string;
  /** What was typed, as typed: the caller keeps it to a code's digits. */
  onType: (raw: string) => void;
  disabled: boolean;
  /** Off for a second code in one form, so the browser fills one field. */
  autoFill?: boolean;
}) {
  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        inputMode="numeric"
        autoComplete={autoFill ? "one-time-code" : undefined}
        maxLength={CONFIRMATION_CODE_LENGTH}
        value={value}
        onChange={(e) => onType(e.target.value)}
        placeholder="000000"
        disabled={disabled}
      />
    </div>
  );
}

/** The label a code mailed to the person's own address goes under. */
export function CodeSentTo({ email }: { email: string }) {
  return (
    <>
      Enter the {CONFIRMATION_CODE_LENGTH}-digit code sent to{" "}
      <span className="font-medium">{email}</span>
    </>
  );
}

/** Resending and confirming share one busy state across the code forms. */
export function ResendCodeButton({ sending, confirming, resendIn, onResend }: {
  sending: boolean;
  confirming: boolean;
  /** Seconds before another code may be asked for. */
  resendIn: number;
  onResend: () => void;
}) {
  return (
    <Button
      type="button"
      variant="outline"
      className="w-fit"
      disabled={sending || confirming || resendIn > 0}
      onClick={onResend}
    >
      {sending ? "Sending…" : resendIn > 0 ? `Resend code in ${resendIn}s` : "Resend code"}
    </Button>
  );
}
