import { ProblemType } from "@/lib/api/problem";

// Why a sign-in ended without a session, as `/auth/refused` words it: the
// callback names one of these, and the page reads nothing else, so no
// address can put words of its own on it.
export const REFUSALS = ["closed", "taken", "unverified", "provider", "expired", "failed"] as const;
export type Refusal = (typeof REFUSALS)[number];

/** A code exchange auth refused: its status, and its problem's type. */
export class ExchangeRefused extends Error {
  constructor(
    readonly status: number,
    readonly type: string | null,
  ) {
    super(`auth exchange failed: ${status}`);
    this.name = "ExchangeRefused";
  }
}

/** The reason an exchange that threw is told as. */
export function refusalOf(err: unknown): Refusal {
  if (err instanceof ExchangeRefused) {
    if (err.type === ProblemType.SignUpClosed) return "closed";
    if (err.status === 409) return "taken";
  }
  return "failed";
}

/** A query's `reason`, read as one the page words, or the plain failure. */
export function refusalNamed(reason: string | undefined): Refusal {
  return REFUSALS.find((known) => known === reason) ?? "failed";
}

/** Where the callback sends a sign-in that ended with no session. */
export function refusedUrl(reason: Refusal, base: string): URL {
  return new URL(`/auth/refused?reason=${reason}`, base);
}
