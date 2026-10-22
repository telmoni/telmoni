"use server";

import { refresh } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { MonitorSchema, type Monitor } from "@/lib/server/entities/monitors";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import { activeProjectForMutation } from "@/lib/server/identity";
import { getServerSession } from "@/lib/server/session";

// The monitor pages' writes, each forwarded to telmoni's monitor lanes as
// the person, who must own or administer the project there: the lane, not
// this file, decides who may, and checks every field. A body is held to the
// lane's shape first, so nothing else is forwarded.

const SLUG = /^[a-z0-9]+(-[a-z0-9]+)*$/;

const BodySchema = z
  .object({
    name: z.string().max(200),
    slug: z.string().max(200).optional(),
    agent: z.string().max(400).optional(),
    schedule: z.string().max(400).optional(),
    interval_s: z.number().int().optional(),
    timezone: z.string().max(200).optional(),
    grace_s: z.number().int().optional(),
    expected_duration_s: z.number().int().optional(),
    bounds: z
      .object({
        stall_after_s: z.number().int().optional(),
        max_steps: z.number().int().optional(),
        max_cost_usd: z.number().optional(),
      })
      .strict(),
  })
  .strict();

const TimingSchema = z
  .object({
    schedule: z.string().max(400).optional(),
    interval_s: z.number().int().optional(),
    timezone: z.string().max(200).optional(),
  })
  .strict();

const PreviewSchema = z.object({ next: z.array(z.string()) });
const MintedSchema = z.object({ key: z.string(), prefix: z.string() });

export type MonitorResult =
  | { status: "ok"; monitor: Monitor }
  | { status: "error"; error: string };

export type PreviewResult =
  | { status: "ok"; next: string[] }
  | { status: "error"; error: string };

export type MintedPingKey =
  | { status: "ok"; key: string; prefix: string }
  | { status: "error"; error: string };

type Lane = { method: string; path: string; body?: unknown };

const SIGNED_OUT = "Your session expired — sign in again.";
const UNREACHABLE = "The monitor service is unreachable. Try again.";

/** One write on telemetry's lanes as the signed-in person, within `limit` a
 *  minute: the answer, or what to tell them. */
async function send(
  projectId: string,
  bucket: string,
  limit: number,
  lane: Lane,
): Promise<{ res: Response } | { error: string }> {
  const session = await getServerSession();
  if (!session) return { error: SIGNED_OUT };
  if (await rateLimit(sessionKey(session, bucket), { limit, windowMs: 60_000 })) {
    return { error: "Too many changes at once. Try again in a minute." };
  }
  const resolved = await activeProjectForMutation(projectId);
  if (resolved.error !== null) return { error: resolved.error };
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry${lane.path}`, {
    method: lane.method,
    headers: {
      ...sessionHeaders(session, resolved.organizationId, resolved.projectId),
      ...(lane.body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(lane.body === undefined ? {} : { body: JSON.stringify(lane.body) }),
  });
  if (!res) return { error: UNREACHABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return { res };
}

/** A write that answers the monitor as it now stands. */
async function monitorWrite(
  projectId: string,
  bucket: string,
  lane: Lane,
): Promise<MonitorResult> {
  const sent = await send(projectId, bucket, 30, lane);
  if ("error" in sent) return { status: "error", error: sent.error };
  // The change is made whatever its answer reads as: the pages draw again.
  refresh();
  const parsed = MonitorSchema.safeParse(await sent.res.json().catch(() => null));
  if (!parsed.success) return { status: "error", error: "Unexpected answer from the monitor service." };
  return { status: "ok", monitor: parsed.data };
}

function monitorPath(slug: string): string | null {
  return SLUG.test(slug) ? `/monitors/${slug}` : null;
}

export async function createMonitorAction(
  projectId: string,
  input: unknown,
): Promise<MonitorResult> {
  const body = BodySchema.safeParse(input);
  if (!body.success) return { status: "error", error: "That monitor could not be read." };
  return monitorWrite(projectId, "monitors:write", {
    method: "POST",
    path: "/monitors",
    body: body.data,
  });
}

export async function updateMonitorAction(
  projectId: string,
  slug: string,
  input: unknown,
): Promise<MonitorResult> {
  const path = monitorPath(slug);
  const body = BodySchema.safeParse(input);
  if (!path || !body.success) return { status: "error", error: "That monitor could not be read." };
  return monitorWrite(projectId, "monitors:write", { method: "PUT", path, body: body.data });
}

export async function pauseMonitorAction(projectId: string, slug: string): Promise<MonitorResult> {
  const path = monitorPath(slug);
  if (!path) return { status: "error", error: "That monitor could not be read." };
  return monitorWrite(projectId, "monitors:write", { method: "POST", path: `${path}/pause` });
}

export async function resumeMonitorAction(projectId: string, slug: string): Promise<MonitorResult> {
  const path = monitorPath(slug);
  if (!path) return { status: "error", error: "That monitor could not be read." };
  return monitorWrite(projectId, "monitors:write", { method: "POST", path: `${path}/resume` });
}

export async function deleteMonitorAction(
  projectId: string,
  slug: string,
): Promise<{ error?: string }> {
  const path = monitorPath(slug);
  if (!path) return { error: "That monitor could not be read." };
  const sent = await send(projectId, "monitors:write", 30, { method: "DELETE", path });
  if ("error" in sent) return { error: sent.error };
  refresh();
  return {};
}

/** The next times a schedule would be due, as it is typed: saves nothing. */
export async function previewScheduleAction(
  projectId: string,
  input: unknown,
): Promise<PreviewResult> {
  const body = TimingSchema.safeParse(input);
  if (!body.success) return { status: "error", error: "That schedule could not be read." };
  const sent = await send(projectId, "monitors:preview", 120, {
    method: "POST",
    path: "/monitors/schedule",
    body: body.data,
  });
  if ("error" in sent) return { status: "error", error: sent.error };
  const parsed = PreviewSchema.safeParse(await sent.res.json().catch(() => null));
  if (!parsed.success) return { status: "error", error: "Unexpected answer from the monitor service." };
  return { status: "ok", next: parsed.data.next };
}

/** A new ping key of the project, shown this once. */
export async function mintPingKeyAction(projectId: string): Promise<MintedPingKey> {
  const sent = await send(projectId, "ping-keys:mint", 5, { method: "POST", path: "/ping-keys" });
  if ("error" in sent) return { status: "error", error: sent.error };
  // The key exists from here on, whatever its answer reads as.
  refresh();
  const parsed = MintedSchema.safeParse(await sent.res.json().catch(() => null));
  if (!parsed.success) return { status: "error", error: "Unexpected answer from the key service." };
  return { status: "ok", key: parsed.data.key, prefix: parsed.data.prefix };
}

/** One of the project's ping keys, retired now. */
export async function retirePingKeyAction(
  projectId: string,
  keyId: string,
): Promise<{ error?: string }> {
  if (!z.guid().safeParse(keyId).success) return { error: "Invalid key id." };
  const sent = await send(projectId, "ping-keys:retire", 10, {
    method: "DELETE",
    path: `/ping-keys/${keyId}`,
  });
  if ("error" in sent) return { error: sent.error };
  refresh();
  return {};
}
