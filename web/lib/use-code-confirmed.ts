"use client";

import { useState } from "react";

import type { ActionResult } from "@/lib/action-result";
import { codeTyped, isConfirmationCode } from "@/lib/confirmation-code";
import { useAction } from "@/lib/use-action";

/** Where a change confirmed by an emailed code stands: nothing asked yet, a
 *  code sent and awaited, or done with what the confirmation answered. */
export type CodeStep<Done> = { step: "ask" } | { step: "code" } | { step: "done"; done: Done };

/** One change confirmed by an emailed code: `send` mails the code, `confirm`
 *  makes the change with it. One error line under both steps, either clearing
 *  what the other said. */
export function useCodeConfirmed<Done extends object>(
  send: () => Promise<ActionResult>,
  confirm: (code: string) => Promise<ActionResult<Done>>,
) {
  const [step, setStep] = useState<CodeStep<{ ok: true } & Done>>({ step: "ask" });
  const [code, setCode] = useState("");
  const sending = useAction();
  const confirming = useAction();
  const clear = () => {
    sending.setError(null);
    confirming.setError(null);
  };

  return {
    step,
    code,
    error: confirming.error ?? sending.error,
    sending: sending.pending,
    confirming: confirming.pending,
    canConfirm: step.step === "code" && isConfirmationCode(code) && !confirming.pending,
    type(raw: string) {
      setCode(codeTyped(raw));
      clear();
    },
    sendCode() {
      clear();
      sending.run(send, () => setStep({ step: "code" }));
    },
    confirmCode() {
      clear();
      confirming.run(
        () => confirm(code),
        (done) => setStep({ step: "done", done }),
      );
    },
  };
}
