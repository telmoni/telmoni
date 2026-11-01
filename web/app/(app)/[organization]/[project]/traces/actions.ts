"use server";

import { refresh } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { MAX_LIMIT } from "@/lib/list-query";
import { type Ceiling, admitProject } from "@/lib/server/admit";
import {
  SessionLayoutSettingSchema,
  type SessionLayoutSetting,
} from "@/lib/server/entities/session-layout";
import {
  LayoutQuerySchema,
  SavedViewOfAnyKindSchema,
  SessionViewQuerySchema,
  TraceTagsSchema,
  UserViewQuerySchema,
  ViewQuerySchema,
  type ListView,
  type TraceLayout,
} from "@/lib/server/entities/traces";
import { pathAndQuery, requestUrl } from "@/lib/server/path";
import { SessionColumn, TraceColumn, UserColumn, ViewKind } from "@/lib/types/enums";

// A project's saved views and layouts, saved, saved over and deleted; the
// layout it opens in; a run's tags and its deletion; an end user's erasure:
// each through its lane in telemetry, which holds the role each takes and
// checks every value. What this checks is the shape the browser sent.

export type ViewResult = ActionResult<{ view?: ListView | TraceLayout }>;

const name = z.string().max(200);
const columns = z.array(z.enum(TraceColumn)).max(64);

/** What a view's save sends: what the browser could send, before the server
 *  checks its every value — a list's query and columns, or a layout's
 *  options, as its kind says. */
const ViewInputSchema = z.discriminatedUnion("kind", [
  z.object({ name, kind: z.literal(ViewKind.Traces), query: ViewQuerySchema, columns }),
  z.object({
    name,
    kind: z.literal(ViewKind.Sessions),
    query: SessionViewQuerySchema,
    columns: z.array(z.enum(SessionColumn)).max(64),
  }),
  z.object({
    name,
    kind: z.literal(ViewKind.Users),
    query: UserViewQuerySchema,
    columns: z.array(z.enum(UserColumn)).max(64),
  }),
  z.object({ name, kind: z.literal(ViewKind.Layout), query: LayoutQuerySchema, columns }),
]);

/** What setting the project's layout sends: a saved layout, or `null` for
 *  the default. */
const SessionLayoutInputSchema = z.strictObject({ view_id: z.guid().nullable() });

/** Saves a person makes in a minute: far more than a hand makes, few enough
 *  that a loop cannot fill a project's hundred views in one. */
const SAVES: Ceiling = { call: "traces:views", limit: 20 };

/** Deletions a person asks for in a minute: each one a mark the span store
 *  queues, so a hand's pace and no loop's. */
const DELETIONS: Ceiling = { call: "traces:delete", limit: 10 };

/** How long a deletion is waited for: its reads and marks each have ten
 *  seconds, and the server finishes it if the console stops waiting. */
const DELETION_WAIT_MS = 60_000;

/** Changes to runs' tags a person asks for in a minute, a session's every
 *  trace one of them: a hand's pace. */
const TAGGINGS: Ceiling = { call: "traces:tags", limit: 30 };

/** How long a change to a run's tags is waited for: its read and its patch
 *  each have ten seconds. */
const TAGGING_WAIT_MS = 30_000;

type Method = "POST" | "PUT" | "PATCH" | "DELETE";

/** One of telemetry's lanes under `/internal/v1/telemetry`, called as the
 *  person: the answer, or why it was not sent or was refused. */
async function call(
  method: Method,
  lane: string,
  projectId: string,
  body?: unknown,
  ceiling: Ceiling = SAVES,
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

async function send(
  method: "POST" | "PUT" | "DELETE",
  path: string,
  projectId: string,
  body?: z.infer<typeof ViewInputSchema>,
): Promise<ViewResult> {
  const sent = await call(method, `/views${path}`, projectId, body);
  if (!sent.ok) return sent;
  if (sent.res.status === 204) return { ok: true };
  const parsed = SavedViewOfAnyKindSchema.safeParse(await sent.res.json().catch(() => null));
  if (!parsed.success) {
    // Saved all the same: the page is drawn again, its menu with it.
    refresh();
    return refused("Unexpected response from telemetry.");
  }
  return { ok: true, view: parsed.data };
}

/** A new view of the project's list, or a new layout, which the page then
 *  opens on: the page it was saved from, with the view named, drawn with
 *  this answer. */
export async function saveTraceViewAction(projectId: string, input: unknown): Promise<ViewResult> {
  const parsed = ViewInputSchema.safeParse(input);
  if (!parsed.success) return refused("That view could not be read.");
  const result = await send("POST", "", projectId, parsed.data);
  if (!result.ok) return result;
  if (!result.view) {
    refresh();
    return result;
  }
  const here = await requestUrl();
  here.searchParams.set("view", result.view.id);
  redirect(pathAndQuery(here));
}

/** One of the project's views, saved over with the list as it is now. */
export async function updateTraceViewAction(
  projectId: string,
  viewId: string,
  input: unknown,
): Promise<ViewResult> {
  if (!z.guid().safeParse(viewId).success) return refused("Invalid view id.");
  const parsed = ViewInputSchema.safeParse(input);
  if (!parsed.success) return refused("That view could not be read.");
  const result = await send("PUT", `/${viewId}`, projectId, parsed.data);
  // Saved over or renamed: the list is drawn again, its views menu with it.
  if (result.ok) refresh();
  return result;
}

/** One of the project's views, gone. A page opened from it goes back to the
 *  list it showed, without it; any other is drawn again, its menu without it. */
export async function deleteTraceViewAction(projectId: string, viewId: string): Promise<ViewResult> {
  if (!z.guid().safeParse(viewId).success) return refused("Invalid view id.");
  const result = await send("DELETE", `/${viewId}`, projectId);
  if (!result.ok) return result;
  const here = await requestUrl();
  if (here.searchParams.get("view") !== viewId) {
    refresh();
    return result;
  }
  here.searchParams.delete("view");
  redirect(pathAndQuery(here));
}

export type SessionLayoutResult = ActionResult<{ setting: SessionLayoutSetting }>;

/** What the project's sessions and traces open in, set: one of its saved
 *  layouts, or the default. The page is drawn again with it. */
export async function setSessionLayoutAction(
  projectId: string,
  input: unknown,
): Promise<SessionLayoutResult> {
  const parsed = SessionLayoutInputSchema.safeParse(input);
  if (!parsed.success) return refused("That layout could not be read.");
  const sent = await call("PUT", "/session-layout", projectId, parsed.data);
  if (!sent.ok) return sent;
  const setting = SessionLayoutSettingSchema.safeParse(await sent.res.json().catch(() => null));
  refresh();
  if (!setting.success) return refused("Unexpected response from telemetry.");
  return { ok: true, setting: setting.data };
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

/** What a change to a run's tags sends: the tags to set, by key, and the
 *  keys to take off. */
const TagsChangeSchema = z.strictObject({
  add: z.record(z.string(), z.string()).optional(),
  remove: z.array(z.string()).optional(),
});

/** The tag set on each of a session's runs, and the runs, by their ids: as
 *  many as its page shows. */
const TagSchema = z.strictObject({ key: z.string(), value: z.string() });
const RunIdsSchema = z.array(z.guid()).min(1).max(MAX_LIMIT);

export type TagsResult = ActionResult<{ tags: Record<string, string> }>;

/** A run's tags changed on its every span, by the run's id: the keys
 *  `remove` names taken off, then `add` set; the page is drawn again. */
export async function setTraceTagsAction(
  projectId: string,
  runId: string,
  change: unknown,
): Promise<TagsResult> {
  if (!z.guid().safeParse(runId).success) return refused("Invalid trace id.");
  const parsed = TagsChangeSchema.safeParse(change);
  if (!parsed.success) return refused("That change to the tags could not be read.");
  const sent = await call(
    "PATCH",
    `/traces/${runId}/tags`,
    projectId,
    parsed.data,
    TAGGINGS,
    TAGGING_WAIT_MS,
  );
  if (!sent.ok) return sent;
  const tags = TraceTagsSchema.safeParse(await sent.res.json().catch(() => null));
  refresh();
  if (!tags.success) return refused("Unexpected response from telemetry.");
  return { ok: true, tags: tags.data.tags };
}

export type TaggedResult = ActionResult<{ tagged: number }>;

/** One tag set on each of `runIds` in turn, the first refusal stopping the
 *  rest: how many took it, or how many had before it. */
export async function tagTracesAction(
  projectId: string,
  runIds: unknown,
  tag: unknown,
): Promise<TaggedResult> {
  const runs = RunIdsSchema.safeParse(runIds);
  const parsed = TagSchema.safeParse(tag);
  if (!runs.success || !parsed.success) return refused("That tag could not be read.");
  const gate = await admitProject(TAGGINGS, projectId);
  if (!gate.ok) return refused(gate.error);
  const add = { [parsed.data.key]: parsed.data.value };
  let tagged = 0;
  for (const runId of runs.data) {
    const sent = await callAs(gate.headers, "PATCH", `/traces/${runId}/tags`, { add }, TAGGING_WAIT_MS);
    if (!sent.ok) {
      if (tagged === 0) return sent;
      refresh();
      return refused(
        `Tagged ${tagged} of ${runs.data.length} traces before one was refused: ${sent.error}`,
      );
    }
    tagged += 1;
  }
  refresh();
  return { ok: true, tagged };
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
