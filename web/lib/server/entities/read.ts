import "server-only";

import type { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { logger } from "@/lib/logger";

/** A lane's answer held to its schema, or why there is none: a status the
 *  caller named, forbidden, or unavailable. */
export type Read<T, E extends string = never> =
  | { kind: "ok"; value: T }
  | { kind: "forbidden" | "unavailable" | E };

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
    const named = statuses[res.status];
    if (named !== undefined) return { kind: named };
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) {
      logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
      return { kind: "unavailable" };
    }
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
