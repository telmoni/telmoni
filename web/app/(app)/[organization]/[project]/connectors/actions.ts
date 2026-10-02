"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit, sessionKey } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { isNotificationKind } from "@/lib/notification-kinds";
import { fetchProject, identityContext, projectHeaders } from "@/lib/server/data";
import {
  ConnectionSchema,
  DeliveryLogPageSchema,
  type DeliveryLogPage,
} from "@/lib/server/entities/connectors";
import { getServerSession } from "@/lib/server/session";
import type { NotificationKind } from "@/lib/types/enums";

export interface ActionResult {
  error: string | null;
}

// How long a test send or a resend is waited for. The service gives the far
// end 10 seconds and the key service 5, around two transactions; waiting only
// the default 10 here reported "unavailable" for a send that went out.
const SEND_WAIT_MS = 20_000;

const UNAVAILABLE = "Connectors are unavailable right now — try again shortly.";
const UNEXPECTED = "Unexpected response from the connector service.";

// A server action's arguments are whatever the browser posted, not what its
// type says, so the kinds are checked against the generated vocabulary before
// they reach the service. `null` is every kind; an empty list would be a
// webhook that receives nothing, which the service refuses too.
function checkKinds(
  eventKinds: readonly NotificationKind[] | null,
): { kinds: NotificationKind[] | null; error?: undefined } | { error: string } {
  if (eventKinds === null) return { kinds: null };
  if (!Array.isArray(eventKinds)) return { error: "Choose which events to send." };
  if (eventKinds.length === 0) return { error: "Choose at least one event." };
  if (!eventKinds.every(isNotificationKind)) return { error: "Unknown event kind." };
  return { kinds: [...new Set(eventKinds)] };
}

// The three checks every action here makes before it spends a request: a
// live session, a ceiling, and a project the caller is on. The role is auth's
// to derive from the bearer and the service's to decide on; nothing here
// asserts it. `connectionId` is `null` for the one action that creates a row
// rather than naming one.
async function open(
  action: string,
  limit: number,
  projectId: string,
  connectionId: string | null,
): Promise<
  | { base: string; headers: Record<string, string>; error?: undefined }
  | { error: string }
> {
  const session = await getServerSession();
  if (!session) return { error: "Your session expired — sign in again." };
  const limited = await rateLimit(sessionKey(session, action), {
    limit,
    windowMs: 60_000,
  });
  if (limited) return { error: "Too many requests — slow down a moment." };
  if (connectionId !== null && !z.guid().safeParse(connectionId).success) {
    return { error: "Invalid connection." };
  }
  const base = env.SERVER_URL;
  const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
  if (!ctx || !project) return { error: "Couldn't resolve this project. Reload and try again." };
  return { base, headers: projectHeaders(ctx, projectId) };
}

export async function disconnectConnectorAction(
  projectId: string,
  connectionId: string,
): Promise<ActionResult> {
  const gate = await open("connectors:disconnect", 10, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok && res.status !== 404) return { error: (await extractProblem(res)).message };
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return { error: null };
}

export interface TestResult extends ActionResult {
  delivered?: boolean;
}

// The service posts now and reports the vendor's answer; a refusal that
// names the connection retires it there, so the page re-reads afterwards.
export async function sendTestMessageAction(
  projectId: string,
  connectionId: string,
): Promise<TestResult> {
  const gate = await open("connectors:test", 10, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}/test`,
    { method: "POST", headers: gate.headers },
    SEND_WAIT_MS,
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  const body = (await res.json().catch(() => null)) as
    | { delivered?: boolean; error?: string | null }
    | null;
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  if (body?.delivered === true) return { error: null, delivered: true };
  return {
    error: body?.error || "The message was not delivered.",
    delivered: false,
  };
}

// What the service answers when it mints a signing secret: the row, and the
// secret this one time. The host is the one thing the page will show of the
// endpoint, so it rides along to title the dialog.
const MintedSecret = z.object({
  connection: z.object({ id: z.string(), channel_name: z.string() }),
  signing_secret: z.string().min(1),
});

export interface SecretResult extends ActionResult {
  signingSecret?: string;
  host?: string;
}

async function readSecret(res: Response): Promise<SecretResult> {
  const parsed = MintedSecret.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return { error: UNEXPECTED };
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return {
    error: null,
    signingSecret: parsed.data.signing_secret,
    host: parsed.data.connection.channel_name,
  };
}

// The one connector connected by a URL. The URL goes to the service as typed:
// the service is the validator — https, no credentials, a host it will dial —
// and a second copy of that rule here is how two validators start to disagree.
// Its refusal comes back as the problem's own sentence, which the dialog
// renders as it is.
export async function createWebhookConnectorAction(
  projectId: string,
  url: string,
  eventKinds: NotificationKind[] | null,
): Promise<SecretResult> {
  const gate = await open("connectors:webhook", 10, projectId, null);
  if (gate.error !== undefined) return { error: gate.error };
  const trimmed = url.trim();
  if (trimmed.length === 0) return { error: "Enter the endpoint URL." };
  if (trimmed.length > 2048) return { error: "That URL is too long." };
  const checked = checkKinds(eventKinds);
  if (checked.error !== undefined) return { error: checked.error };
  const res = await tryFetchWithTimeout(`${gate.base}/internal/connectors/webhook`, {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ url: trimmed, event_kinds: checked.kinds }),
  });
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return readSecret(res);
}

const UpdatedConnection = z.object({ connection: ConnectionSchema });

// Which notices an endpoint receives. A webhook's alone: Slack and Discord
// take every kind, and the service refuses the change for them.
export async function updateWebhookEventsAction(
  projectId: string,
  connectionId: string,
  eventKinds: NotificationKind[] | null,
): Promise<ActionResult> {
  const gate = await open("connectors:events", 20, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  const checked = checkKinds(eventKinds);
  if (checked.error !== undefined) return { error: checked.error };
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}/events`,
    {
      method: "PUT",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ event_kinds: checked.kinds }),
    },
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  const parsed = UpdatedConnection.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return { error: UNEXPECTED };
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return { error: null };
}

// A new signing secret, shown once, and an errored webhook comes back active
// with it — rotation is the webhook's Reconnect. For the hours asked, the
// service signs each delivery with the old secret as well, so a receiver can
// swap at its own pace; `0` is the hard cut a leaked secret needs. The body is
// always sent: an absent one would leave the overlap to the service's default,
// which is not a choice the owner made.
export async function rotateWebhookSecretAction(
  projectId: string,
  connectionId: string,
  keepPreviousForHours: number,
): Promise<SecretResult> {
  const gate = await open("connectors:rotate", 10, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  if (
    !Number.isInteger(keepPreviousForHours) ||
    keepPreviousForHours < 0 ||
    keepPreviousForHours > 24
  ) {
    return { error: "Choose how long the current secret keeps working, from 0 to 24 hours." };
  }
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}/rotate`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ keep_previous_for_hours: keepPreviousForHours }),
    },
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  return readSecret(res);
}

export interface DeliveryLogResult extends ActionResult {
  page?: DeliveryLogPage;
}

const DELIVERY_PAGE_SIZE = 25;

// A read, so every role on the project gets it; the service decides that
// from the bearer as it does for the listing. The ceiling is higher than the
// writes' because paging back through a busy log is several reads in a row.
export async function listDeliveriesAction(
  projectId: string,
  connectionId: string,
  before: string | null,
): Promise<DeliveryLogResult> {
  const gate = await open("connectors:deliveries", 60, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  if (before !== null && !z.guid().safeParse(before).success) {
    return { error: "Invalid delivery." };
  }
  const query = new URLSearchParams({ limit: String(DELIVERY_PAGE_SIZE) });
  if (before !== null) query.set("before", before);
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}/deliveries?${query}`,
    { method: "GET", headers: gate.headers },
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  const parsed = DeliveryLogPageSchema.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return { error: UNEXPECTED };
  return { error: null, page: parsed.data };
}

const Redelivered = z.object({
  delivered: z.boolean(),
  error: z.string().nullable(),
});

// Sends the same delivery once more, now, under the same id — the receiver's
// dedup on `Telmoni-Delivery-Id` is what makes a resend of a notice it did
// get harmless. Like a test, a refusal that names the connection retires it
// upstream, so the page re-reads either way.
export async function redeliverAction(
  projectId: string,
  connectionId: string,
  deliveryId: string,
): Promise<TestResult> {
  const gate = await open("connectors:redeliver", 10, projectId, connectionId);
  if (gate.error !== undefined) return { error: gate.error };
  if (!z.guid().safeParse(deliveryId).success) {
    return { error: "Invalid delivery." };
  }
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/connectors/${encodeURIComponent(connectionId)}/deliveries/${encodeURIComponent(deliveryId)}/redeliver`,
    { method: "POST", headers: gate.headers },
    SEND_WAIT_MS,
  );
  if (!res) return { error: UNAVAILABLE };
  if (!res.ok) return { error: (await extractProblem(res)).message };
  const parsed = Redelivered.safeParse(await res.json().catch(() => null));
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  if (!parsed.success) return { error: UNEXPECTED };
  if (parsed.data.delivered) return { error: null, delivered: true };
  return {
    error: parsed.data.error || "The delivery failed again.",
    delivered: false,
  };
}
