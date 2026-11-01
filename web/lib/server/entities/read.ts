import "server-only";

import type { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { logger } from "@/lib/logger";

/** A lane's answer held to its schema, or why there is none: a status the
 *  caller named, forbidden, or unavailable. */
export type Read<T, E extends string = never> =
  | { kind: "ok"; value: T }
  | { kind: "forbidden" | "unavailable" | E };

/** A lane's bytes and their type, or why there are none, as a [`Read`]. */
export type ReadBytes<E extends string = never> =
  | { kind: "ok"; body: ArrayBuffer; contentType: string }
  | { kind: "forbidden" | "unavailable" | E };

/** Why `res` holds no answer, as every reader words it: a status `statuses`
 *  names, which logs nothing, forbidden, or unavailable, logged by its
 *  status; `null` where it answered. */
function refusal<E extends string>(
  fetcher: string,
  res: Response,
  statuses: Readonly<Partial<Record<number, E>>>,
): { kind: "forbidden" | "unavailable" | E } | null {
  const named = statuses[res.status];
  if (named !== undefined) return { kind: named };
  if (res.status === 403) return { kind: "forbidden" };
  if (res.ok) return null;
  logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
  return { kind: "unavailable" };
}

/** One server read, logged as every entity logs one: a refusal by its status,
 *  an answer of the wrong shape by its issues, a failure by its message. A
 *  status `statuses` names is the caller's to word, and logs nothing. */
export async function readJson<T, E extends string = never>(
  fetcher: string,
  url: string,
  init: RequestInit,
  schema: z.ZodType<T>,
  statuses: Readonly<Partial<Record<number, E>>> = {},
): Promise<Read<T, E>> {
  try {
    const res = await fetchWithTimeout(url, init);
    const refused = refusal(fetcher, res, statuses);
    if (refused) return refused;
    const parsed = schema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn({ fetcher, issues: parsed.error.issues }, "entities: upstream shape mismatch");
      return { kind: "unavailable" };
    }
    return { kind: "ok", value: parsed.data };
  } catch (err) {
    readFailed(fetcher, err);
    return { kind: "unavailable" };
  }
}

/** One server read of bytes, passed through as they came, refused and logged
 *  as [`readJson`] does. */
export async function readBytes<E extends string = never>(
  fetcher: string,
  url: string,
  init: RequestInit,
  statuses: Readonly<Partial<Record<number, E>>> = {},
  timeoutMs?: number,
): Promise<ReadBytes<E>> {
  try {
    const res = await fetchWithTimeout(url, init, timeoutMs);
    const refused = refusal(fetcher, res, statuses);
    if (refused) return refused;
    return {
      kind: "ok",
      body: await res.arrayBuffer(),
      contentType: res.headers.get("content-type") ?? "application/octet-stream",
    };
  } catch (err) {
    readFailed(fetcher, err);
    return { kind: "unavailable" };
  }
}

/** A read that threw, by its message alone: the binary readers' too. */
export function readFailed(fetcher: string, err: unknown): void {
  logger.warn(
    { fetcher, error: err instanceof Error ? err.message : String(err) },
    "entities: fetch failed",
  );
}

/** The value of a read that answered, else `null`: for a fetcher whose page
 *  shows one notice for every way a read can fail. */
export const valueOf = <T, E extends string>(read: Read<T, E>): T | null =>
  "value" in read ? read.value : null;
