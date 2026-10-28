"use server";

import { refresh } from "next/cache";
import { z } from "zod";

import { refused } from "@/lib/action-result";
import { MUTE_MINUTES_MAX } from "@/lib/alerts";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";
import {
  MissingData,
  RuleAggregation,
  RuleLevel,
  RuleMeasure,
  RuleOperator,
  RuleWindow,
  SpanKind,
  SpanStatus,
} from "@/lib/types/enums";

import type { Answer } from "./_act";

// Every change the alerts page makes, each open to any member of the project
// (`Resource::Rule`): an alert made, saved over, paused, resumed or deleted;
// a connector it muted after five failed deliveries heard again; an incident
// acknowledged or resolved; and a maintenance window made or ended. A Server
// Action takes whatever is posted to it, so each id is checked to be one
// before it becomes part of a lane's path, and an alert's shape before it is
// sent; the lane checks the rest and says what it refused.

type Result = Answer;

const Id = z.guid();
const Values = z.array(z.string()).optional();

const RuleInputSchema = z.strictObject({
  name: z.string(),
  kind: z.enum([SpanKind.Run, SpanKind.Step, SpanKind.ModelCall, SpanKind.ToolCall]),
  measure: z.enum(RuleMeasure),
  aggregation: z.enum(RuleAggregation),
  filters: z.strictObject({
    agent: Values,
    model: Values,
    provider: Values,
    tool: Values,
    status: z.array(z.enum(SpanStatus)).optional(),
    environment: Values,
    release: Values,
  }),
  window: z.enum(RuleWindow),
  operator: z.enum(RuleOperator),
  alert_threshold: z.number(),
  warning_threshold: z.number().nullable(),
  missing_data: z.enum(MissingData),
  renotify_minutes: z.number().int().nullable(),
  connectors: z.array(
    z.strictObject({
      connection_id: Id,
      level: z.enum(RuleLevel),
      hours: z
        .strictObject({
          days: z.array(z.number().int()),
          from: z.string(),
          to: z.string(),
          utc_offset: z.number().int(),
        })
        .nullable(),
    }),
  ),
});

const MuteInputSchema = z.strictObject({
  rule_id: Id.nullable(),
  starts_at: z.iso.datetime({ offset: true }).nullable(),
  minutes: z.number().int().min(1).max(MUTE_MINUTES_MAX),
});

const UNAVAILABLE = "Alerts are unavailable right now — try again shortly.";

/** One call to an alerts lane on the person's behalf: their session, the
 *  minute's room under `bucket`, and the project resolved, then the lane's
 *  answer, the page drawn again where it took. */
async function send(
  projectId: string,
  bucket: string,
  path: string,
  init: { method: "POST" | "PUT" | "DELETE"; body?: unknown; goneIsDone?: boolean },
): Promise<Result> {
  const gate = await admitProject({ call: bucket, limit: 30 }, projectId);
  if (!gate.ok) return refused(gate.error);
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry${path}`, {
    method: init.method,
    headers: {
      ...gate.headers,
      ...(init.body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(init.body === undefined ? {} : { body: JSON.stringify(init.body) }),
  });
  if (!res) return refused(UNAVAILABLE);
  if (!res.ok && !(init.goneIsDone && res.status === 404)) {
    const { message } = await extractProblem(res);
    // Gone, as another member deleted it: the page is drawn again without it.
    if (res.status === 404) {
      refresh();
      return { ...refused(message), gone: true };
    }
    return refused(message);
  }
  refresh();
  return { ok: true };
}

/** A new alert of the project, or one of its alerts saved over whole. */
export async function saveAlertAction(
  projectId: string,
  ruleId: string | null,
  input: unknown,
): Promise<Result> {
  if (ruleId !== null && !Id.safeParse(ruleId).success) {
    return refused("Couldn't tell which alert this is. Reload and try again.");
  }
  const rule = RuleInputSchema.safeParse(input);
  if (!rule.success) return refused("That alert could not be read. Reload and try again.");
  return ruleId === null
    ? send(projectId, "alerts:save", "/rules", { method: "POST", body: rule.data })
    : send(projectId, "alerts:save", `/rules/${ruleId}`, { method: "PUT", body: rule.data });
}

/** One of the project's alerts gone, its incidents with it; one gone already,
 *  as another member may have deleted it, is drawn gone. */
export async function deleteAlertAction(projectId: string, ruleId: string): Promise<Result> {
  if (!Id.safeParse(ruleId).success) {
    return refused("Couldn't tell which alert this is. Reload and try again.");
  }
  return send(projectId, "alerts:change", `/rules/${ruleId}`, {
    method: "DELETE",
    goneIsDone: true,
  });
}

/** One of the project's alerts paused, or running again. */
export async function pauseAlertAction(
  projectId: string,
  ruleId: string,
  paused: boolean,
): Promise<Result> {
  if (!Id.safeParse(ruleId).success || typeof paused !== "boolean") {
    return refused("Couldn't tell which alert this is. Reload and try again.");
  }
  return send(projectId, "alerts:change", `/rules/${ruleId}/${paused ? "pause" : "resume"}`, {
    method: "POST",
  });
}

/** A connector an alert muted after five failed deliveries, hearing it again. */
export async function hearAgainAction(
  projectId: string,
  ruleId: string,
  connectionId: string,
): Promise<Result> {
  if (!Id.safeParse(ruleId).success || !Id.safeParse(connectionId).success) {
    return refused("Couldn't tell which connector this is. Reload and try again.");
  }
  return send(projectId, "alerts:change", `/rules/${ruleId}/connectors/${connectionId}/unmute`, {
    method: "POST",
  });
}

/** One of the project's incidents acknowledged, or resolved by hand. */
export async function incidentAction(
  projectId: string,
  incidentId: string,
  act: "acknowledge" | "resolve",
): Promise<Result> {
  if (!Id.safeParse(incidentId).success || (act !== "acknowledge" && act !== "resolve")) {
    return refused("Couldn't tell which incident this is. Reload and try again.");
  }
  return send(projectId, "alerts:incident", `/incidents/${incidentId}/${act}`, { method: "POST" });
}

/** A maintenance window muting one of the project's alerts, or all of them,
 *  for `minutes` from its start: now, unless given. Both ends are spelled on
 *  this clock, so a window as long as the lane allows is never read a moment
 *  longer by the server's. */
export async function muteAction(projectId: string, input: unknown): Promise<Result> {
  const mute = MuteInputSchema.safeParse(input);
  if (!mute.success) return refused("That window could not be read. Reload and try again.");
  const start = mute.data.starts_at === null ? Date.now() : Date.parse(mute.data.starts_at);
  return send(projectId, "alerts:mute", "/mutes", {
    method: "POST",
    body: {
      rule_id: mute.data.rule_id,
      starts_at: new Date(start).toISOString(),
      ends_at: new Date(start + mute.data.minutes * 60_000).toISOString(),
    },
  });
}

/** One of the project's maintenance windows ended now. */
export async function endMuteAction(projectId: string, muteId: string): Promise<Result> {
  if (!Id.safeParse(muteId).success) {
    return refused("Couldn't tell which window this is. Reload and try again.");
  }
  return send(projectId, "alerts:mute", `/mutes/${muteId}/end`, { method: "POST" });
}
