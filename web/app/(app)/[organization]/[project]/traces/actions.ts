"use server";

import { refresh } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { type Ceiling, admitProject } from "@/lib/server/admit";
import {
  SessionLayoutSettingSchema,
  type SessionLayoutSetting,
} from "@/lib/server/entities/session-layout";
import {
  LayoutQuerySchema,
  SavedViewOfAnyKindSchema,
  SessionViewQuerySchema,
  UserViewQuerySchema,
  ViewQuerySchema,
  type ListView,
  type TraceLayout,
} from "@/lib/server/entities/traces";
import { pathAndQuery, requestUrl } from "@/lib/server/path";
import { SessionColumn, TraceColumn, UserColumn, ViewKind } from "@/lib/types/enums";

// A project's saved views of its lists and saved layouts of a session's
// page and a trace's, saved, saved over and deleted through telemetry's
// lanes, which hold the role each takes and check each as its page would
// read it, and the layout the project opens in, set through its lane: what
// this checks is only that the browser sent the shape each has.

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

/** One of telemetry's lanes under `/internal/v1/telemetry`, called as the
 *  person: the answer, or why it was not sent or was refused. */
async function call(
  method: "POST" | "PUT" | "DELETE",
  lane: string,
  projectId: string,
  body?: unknown,
  ceiling: Ceiling = SAVES,
  waitMs?: number,
): Promise<ActionResult<{ res: Response }>> {
  const gate = await admitProject(ceiling, projectId);
  if (!gate.ok) return refused(gate.error);

  const url = `${env.SERVER_URL}/internal/v1/telemetry${lane}`;
  const init = {
    method,
    headers: {
      ...gate.headers,
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
