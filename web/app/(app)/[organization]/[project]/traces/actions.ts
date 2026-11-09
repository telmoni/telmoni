"use server";

import { refresh } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { type Ceiling, admitProject } from "@/lib/server/admit";
import { pathAndQuery, requestUrl } from "@/lib/server/path";
import { COMMENT_MAX, SCORE_CATEGORIES_MAX, SCORE_CATEGORY_MAX } from "@/lib/scores";
import { FILTER_VALUE_MAX, SCORE_NAME_MAX } from "@/lib/traces";
import { ScoreType } from "@/lib/types/enums";

// A run's deletion, an end user's erasure and a member's scores: each through
// its lane in telemetry, which holds the role each takes and checks every
// value. What this checks is the shape the browser sent.

/** Deletions a person asks for in a minute: each one a mark the span store
 *  queues, so a hand's pace and no loop's. */
const DELETIONS: Ceiling = { call: "traces:delete", limit: 10 };

/** How long a deletion is waited for: its reads and marks each have ten
 *  seconds, and the server finishes it if the console stops waiting. */
const DELETION_WAIT_MS = 60_000;

type Method = "POST" | "PUT" | "DELETE";

/** One of telemetry's lanes under `/internal/v1/telemetry`, called as the
 *  person: the answer, or why it was not sent or was refused. */
async function call(
  method: Method,
  lane: string,
  projectId: string,
  body: unknown,
  ceiling: Ceiling,
  waitMs?: number,
): Promise<ActionResult<{ res: Response }>> {
  const gate = await admitProject(ceiling, projectId);
  if (!gate.ok) return refused(gate.error);
  return callAs(gate.headers, method, lane, body, waitMs);
}

/** A lane called with the headers of a person already admitted. */
async function callAs(
  headers: Record<string, string>,
  method: Method,
  lane: string,
  body?: unknown,
  waitMs?: number,
): Promise<ActionResult<{ res: Response }>> {
  const url = `${env.SERVER_URL}/internal/v1/telemetry${lane}`;
  const init = {
    method,
    headers: {
      ...headers,
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  };
  const res = await (waitMs
    ? tryFetchWithTimeout(url, init, waitMs)
    : tryFetchWithTimeout(url, init));
  if (!res) return refused("Telemetry is unreachable. Try again.");
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return refused(message);
  }
  return { ok: true, res };
}

/** One trace deleted, by its run's id: the page then goes back to the list,
 *  where it no longer shows. The receipt is mailed by the server. */
export async function deleteTraceAction(projectId: string, runId: string): Promise<ActionResult> {
  if (!z.guid().safeParse(runId).success) return refused("Invalid trace id.");
  const sent = await call("DELETE", `/traces/${runId}`, projectId, undefined, DELETIONS, DELETION_WAIT_MS);
  if (!sent.ok) return sent;
  const here = await requestUrl();
  here.pathname = here.pathname.replace(/\/[^/]+$/, "");
  here.search = "";
  redirect(pathAndQuery(here));
}

const ErasedSchema = z.object({ traces: z.number().int().nonnegative() });

export type ErasureResult = ActionResult<{ traces: number }>;

/** One end user erased by the id the project's SDK sent, every trace it is
 *  on deleted: how many went. The id goes in the body, never a URL. */
export async function eraseEndUserAction(projectId: string, userId: unknown): Promise<ErasureResult> {
  // Characters, as ingest counts them, not UTF-16's units.
  const parsed = z
    .string()
    .refine((id) => id.length > 0 && [...id].length <= 128)
    .safeParse(userId);
  if (!parsed.success) return refused("An end user's id is 1 to 128 characters.");
  const sent = await call(
    "POST",
    "/users/erase",
    projectId,
    { user_id: parsed.data },
    DELETIONS,
    DELETION_WAIT_MS,
  );
  if (!sent.ok) return sent;
  const erased = ErasedSchema.safeParse(await sent.res.json().catch(() => null));
  if (!erased.success) return refused("Unexpected response from telemetry.");
  return { ok: true, traces: erased.data.traces };
}

/** A person's scores a minute, given and taken back. */
const SCORINGS: Ceiling = { call: "traces:scores", limit: 30 };

/** What a member's score sends: what it scores and what it says, before the
 *  server checks its every rule. */
const ScoreInputSchema = z.object({
  run_id: z.guid().optional(),
  span_id: z.string().max(64).optional(),
  session_id: z.string().max(FILTER_VALUE_MAX.session).optional(),
  name: z.string().min(1).max(SCORE_NAME_MAX),
  data_type: z.enum(ScoreType),
  value: z.number().finite().optional(),
  categories: z.array(z.string().max(SCORE_CATEGORY_MAX)).max(SCORE_CATEGORIES_MAX).optional(),
  comment: z.string().max(COMMENT_MAX).optional(),
});

/** A member's score of a run, one of its spans or a session, given again
 *  replacing their last of that name. */
export async function putScoreAction(projectId: string, score: unknown): Promise<ActionResult> {
  const parsed = ScoreInputSchema.safeParse(score);
  if (!parsed.success) return refused("That score could not be read.");
  const sent = await call("PUT", "/scores", projectId, parsed.data, SCORINGS);
  if (!sent.ok) return sent;
  refresh();
  return { ok: true };
}

/** A member's score taken back: their own, or anyone's by an owner or admin. */
export async function deleteScoreAction(projectId: string, scoreId: string): Promise<ActionResult> {
  if (!z.guid().safeParse(scoreId).success) return refused("Invalid score id.");
  const sent = await call("DELETE", `/scores/${scoreId}`, projectId, undefined, SCORINGS);
  if (!sent.ok) return sent;
  refresh();
  return { ok: true };
}
