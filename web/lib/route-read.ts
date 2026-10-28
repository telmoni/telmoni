import type { z } from "zod";

import { SESSION_EXPIRED } from "@/lib/action-result";
import { problemWords } from "@/lib/api/problem";

// What a read of one of the console's own Route Handlers came to, read the
// same way for every browser read: the answer parsed, never trusted by
// assertion, since a deploy landing while a tab is open answers a shape this
// build cannot read; and a session the proxy turned away before any handler
// ran told apart from a refusal. Pure, so `lib/` tests it on a `Response`.

/** What a read came to. A refusal carries the handler's words, when it gave
 *  any; a read that never reached the handler no status. */
export type RouteRead<T> =
  | { kind: "ok"; value: T }
  | { kind: "signed-out" }
  | { kind: "forbidden"; error: string | null }
  | { kind: "missing"; error: string | null }
  | { kind: "unavailable"; status: number | null; error: string | null };

/** A read that never reached the handler: the network, or a cut-off. */
export const UNREACHED: RouteRead<never> = { kind: "unavailable", status: null, error: null };

/** Whether a response says the session has ended: the proxy's 401, or a
 *  redirect to sign in, never followed, since followed it is the sign-in page. */
export function signedOut(res: Response): boolean {
  return res.type === "opaqueredirect" || res.status === 401;
}

/** `res` as a read of `schema`: its status as the read's kind, its body as
 *  the value where it parses, else as the refusal's words. */
export async function routeReadOf<T>(res: Response, schema: z.ZodType<T>): Promise<RouteRead<T>> {
  if (signedOut(res)) return { kind: "signed-out" };
  const body: unknown = await res.json().catch(() => null);
  const error = problemWords(body);
  if (res.status === 403) return { kind: "forbidden", error };
  if (res.status === 404) return { kind: "missing", error };
  const parsed = res.ok ? schema.safeParse(body) : null;
  if (parsed?.success) return { kind: "ok", value: parsed.data };
  return { kind: "unavailable", status: res.status, error };
}

/** A refusal's words for a person: the session's end, the handler's own,
 *  else `fallback`. */
export function refusalWords(
  read: Exclude<RouteRead<unknown>, { kind: "ok" }>,
  fallback: string,
): string {
  if (read.kind === "signed-out") return SESSION_EXPIRED;
  return read.error ?? fallback;
}
