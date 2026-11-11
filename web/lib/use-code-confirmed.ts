"use client";

import { useEffect, useRef, useState } from "react";

import type { ActionResult } from "@/lib/action-result";
import { RESEND_WAIT_SECONDS, codeTyped, isConfirmationCode } from "@/lib/confirmation-code";
import { useAction } from "@/lib/use-action";

/** The seconds left before another code may be asked for, counted down from
 *  each `start`, made when one went out. */
export function useResendWait() {
  const [left, setLeft] = useState(0);
  const timer = useRef<ReturnType<typeof setInterval>>(undefined);
  useEffect(() => () => clearInterval(timer.current), []);
  return {
    left,
    start() {
      clearInterval(timer.current);
      // Against a deadline, so a hidden tab's slowed timer catches up.
      const until = Date.now() + RESEND_WAIT_SECONDS * 1000;
      setLeft(RESEND_WAIT_SECONDS);
      timer.current = setInterval(() => {
        const secs = Math.max(0, Math.ceil((until - Date.now()) / 1000));
        setLeft(secs);
        if (secs === 0) clearInterval(timer.current);
      }, 1000);
    },
  };
}

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
  const wait = useResendWait();
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
    /** Seconds before another code may be asked for. */
    resendIn: wait.left,
    canConfirm: step.step === "code" && isConfirmationCode(code) && !sending.pending && !confirming.pending,
    type(raw: string) {
      setCode(codeTyped(raw));
      clear();
    },
    sendCode() {
      if (sending.pending || confirming.pending || wait.left > 0 || step.step === "done") return;
      clear();
      sending.run(send, () => {
        setCode("");
        setStep({ step: "code" });
        wait.start();
      });
    },
    confirmCode() {
      if (step.step !== "code" || !isConfirmationCode(code) || sending.pending || confirming.pending) return;
      clear();
      confirming.run(
        () => confirm(code),
        (done) => setStep({ step: "done", done }),
      );
    },
  };
}
