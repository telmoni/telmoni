import type { ReactNode } from "react";

import { Input } from "@/components/ui/input";
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
