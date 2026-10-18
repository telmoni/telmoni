"use server";

import { refresh } from "next/cache";
import { redirect } from "next/navigation";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import {
  SessionLayoutSettingSchema,
  type SessionLayoutSetting,
} from "@/lib/server/entities/session-layout";
import {
  LayoutQuerySchema,
  SavedViewOfAnyKindSchema,
  ViewQuerySchema,
  type TraceLayout,
  type TraceView,
} from "@/lib/server/entities/traces";
import { activeProjectForMutation } from "@/lib/server/identity";
import { pathAndQuery, requestUrl } from "@/lib/server/path";
import { getServerSession } from "@/lib/server/session";
import { TraceColumn, ViewKind } from "@/lib/types/enums";

// A project's saved views of its trace list and saved layouts of a session's
// page and a trace's, saved, saved over and deleted through telemetry's
// lanes, which hold the role each takes and check each as its page would
// read it, and the layout the project opens in, set through its lane: what
// this checks is only that the browser sent the shape each has.

export type ViewResult =
  | { status: "ok"; view?: TraceView | TraceLayout }
  | { status: "error"; error: string };

const name = z.string().max(200);
const columns = z.array(z.enum(TraceColumn)).max(64);

/** What a view's save sends: what the browser could send, before the server
 *  checks its every value — a list's query, or a layout's options, as its
 *  kind says. */
const ViewInputSchema = z.discriminatedUnion("kind", [
  z.object({ name, kind: z.literal(ViewKind.List), query: ViewQuerySchema, columns }),
  z.object({ name, kind: z.literal(ViewKind.Layout), query: LayoutQuerySchema, columns }),
]);

/** What setting the project's layout sends: a saved layout, or `null` for
 *  the default. */
const SessionLayoutInputSchema = z.strictObject({ view_id: z.guid().nullable() });

/** Saves a person makes in a minute: far more than a hand makes, few enough
 *  that a loop cannot fill a project's hundred views in one. */
const SAVES_A_MINUTE = { limit: 20, windowMs: 60_000 };

type Refusal = { status: "error"; error: string };

/** One of telemetry's lanes under `/internal/telemetry`, called as the
 *  person: the answer, or why it was not sent or was refused. */
async function call(
  method: "POST" | "PUT" | "DELETE",
  lane: string,
  projectId: string,
  body?: unknown,
): Promise<{ status: "sent"; res: Response } | Refusal> {
  const session = await getServerSession();
  if (!session) return { status: "error", error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, "traces:views"), SAVES_A_MINUTE);
  if (limited) return { status: "error", error: "Too many saves at once. Try again in a minute." };
  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { status: "error", error: resolved.error };

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/telemetry${lane}`, {
    method,
    headers: {
      ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res) return { status: "error", error: "Telemetry is unreachable. Try again." };
  if (!res.ok) {
    const { message } = await extractProblem(res);
    return { status: "error", error: message };
  }
  return { status: "sent", res };
}

async function send(
  method: "POST" | "PUT" | "DELETE",
  path: string,
  projectId: string,
  body?: z.infer<typeof ViewInputSchema>,
): Promise<ViewResult> {
  const sent = await call(method, `/views${path}`, projectId, body);
  if (sent.status === "error") return sent;
  if (sent.res.status === 204) return { status: "ok" };
  const parsed = SavedViewOfAnyKindSchema.safeParse(await sent.res.json().catch(() => null));
  if (!parsed.success) {
    // Saved all the same: the page is drawn again, its menu with it.
    refresh();
    return { status: "error", error: "Unexpected response from telemetry." };
  }
  return { status: "ok", view: parsed.data };
}

/** A new view of the project's list, or a new layout, which the page then
 *  opens on: the page it was saved from, with the view named, drawn with
 *  this answer. */
export async function saveTraceViewAction(projectId: string, input: unknown): Promise<ViewResult> {
  const parsed = ViewInputSchema.safeParse(input);
  if (!parsed.success) return { status: "error", error: "That view could not be read." };
  const result = await send("POST", "", projectId, parsed.data);
  if (result.status === "error") return result;
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
  if (!z.guid().safeParse(viewId).success) return { status: "error", error: "Invalid view id." };
  const parsed = ViewInputSchema.safeParse(input);
  if (!parsed.success) return { status: "error", error: "That view could not be read." };
  const result = await send("PUT", `/${viewId}`, projectId, parsed.data);
  // Saved over or renamed: the list is drawn again, its views menu with it.
  if (result.status === "ok") refresh();
  return result;
}

/** One of the project's views, gone. A page opened from it goes back to the
 *  list it showed, without it; any other is drawn again, its menu without it. */
export async function deleteTraceViewAction(projectId: string, viewId: string): Promise<ViewResult> {
  if (!z.guid().safeParse(viewId).success) return { status: "error", error: "Invalid view id." };
  const result = await send("DELETE", `/${viewId}`, projectId);
  if (result.status === "error") return result;
  const here = await requestUrl();
  if (here.searchParams.get("view") !== viewId) {
    refresh();
    return result;
  }
  here.searchParams.delete("view");
  redirect(pathAndQuery(here));
}

export type SessionLayoutResult =
  | { status: "ok"; setting: SessionLayoutSetting }
  | { status: "error"; error: string };

/** What the project's sessions and traces open in, set: one of its saved
 *  layouts, or the default. The page is drawn again with it. */
export async function setSessionLayoutAction(
  projectId: string,
  input: unknown,
): Promise<SessionLayoutResult> {
  const parsed = SessionLayoutInputSchema.safeParse(input);
  if (!parsed.success) return { status: "error", error: "That layout could not be read." };
  const sent = await call("PUT", "/session-layout", projectId, parsed.data);
  if (sent.status === "error") return sent;
  const setting = SessionLayoutSettingSchema.safeParse(await sent.res.json().catch(() => null));
  refresh();
  if (!setting.success) return { status: "error", error: "Unexpected response from telemetry." };
  return { status: "ok", setting: setting.data };
}
