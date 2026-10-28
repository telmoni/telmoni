// What a Server Action answers: done, with whatever it made, or refused, with
// the sentence the page shows. Neither carries the other's fields, so a caller
// narrows on `ok` and never re-checks what the type promises.

export type Refused = { ok: false; error: string };

export type ActionResult<T extends object = Record<never, never>> = ({ ok: true } & T) | Refused;

export const refused = (error: string): Refused => ({ ok: false, error });

/** The refusal's words, or `null` when the action was done. */
export const errorOf = (result: ActionResult<object>): string | null =>
  result.ok ? null : result.error;
