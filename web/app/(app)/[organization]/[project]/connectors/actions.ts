"use server";

import { revalidatePath } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { isNotificationKind } from "@/lib/notification-kinds";
import {
  CONNECTORS_UNAVAILABLE,
  CONNECTORS_UNEXPECTED,
  admitConnectorsCall,
} from "@/lib/server/connectors-admit";
import { ConnectionSchema } from "@/lib/server/entities/connectors";
import type { NotificationKind } from "@/lib/types/enums";

// How long a test send or a resend is waited for. The service gives the far
// end 10 seconds and the key service 5, around two transactions; waiting only
// the default 10 here reported "unavailable" for a send that went out.
const SEND_WAIT_MS = 20_000;

/** What a send now answers, a test or a resend: the vendor's refusal, if any. */
const SendAnswer = z.object({
  delivered: z.boolean(),
  error: z.string().nullable(),
});

// A server action's arguments are whatever the browser posted, not what its
// type says, so the kinds are checked against the generated vocabulary before
// they reach the service. `null` is every kind; an empty list would be a
// webhook that receives nothing, which the service refuses too.
function checkKinds(
  eventKinds: readonly NotificationKind[] | null,
): ActionResult<{ kinds: NotificationKind[] | null }> {
  if (eventKinds === null) return { ok: true, kinds: null };
  if (!Array.isArray(eventKinds)) return refused("Choose which events to send.");
  if (eventKinds.length === 0) return refused("Choose at least one event.");
  if (!eventKinds.every(isNotificationKind)) return refused("Unknown event kind.");
  return { ok: true, kinds: [...new Set(eventKinds)] };
}

export async function disconnectConnectorAction(
  projectId: string,
  connectionId: string,
): Promise<ActionResult> {
  const gate = await admitConnectorsCall("connectors:disconnect", 10, projectId, connectionId);
  if (gate.error !== undefined) return refused(gate.error);
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connectionId)}`,
    { method: "DELETE", headers: gate.headers },
  );
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  if (!res.ok && res.status !== 404) return refused((await extractProblem(res)).message);
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return { ok: true };
}

// The service posts now and reports the vendor's answer; a refusal that
// names the connection retires it there, so the page re-reads afterwards.
export async function sendTestMessageAction(
  projectId: string,
  connectionId: string,
): Promise<ActionResult> {
  const gate = await admitConnectorsCall("connectors:test", 10, projectId, connectionId);
  if (gate.error !== undefined) return refused(gate.error);
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connectionId)}/test`,
    { method: "POST", headers: gate.headers },
    SEND_WAIT_MS,
  );
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  if (!res.ok) return refused((await extractProblem(res)).message);
  const sent = SendAnswer.safeParse(await res.json().catch(() => null));
  if (!sent.success) return refused(CONNECTORS_UNEXPECTED);
  if (sent.data.delivered) return { ok: true };
  return refused(sent.data.error || "The message was not delivered.");
}

// What the service answers when it mints a signing secret: the row, and the
// secret this one time. The host is the one thing the page will show of the
// endpoint, so it rides along to title the dialog.
const MintedSecret = z.object({
  connection: z.object({ id: z.string(), channel_name: z.string() }),
  signing_secret: z.string().min(1),
});

/** A signing secret, shown this once, and the host that titles its dialog. */
export type SigningSecret = { signingSecret: string; host: string };
export type SecretResult = ActionResult<SigningSecret>;

async function readSecret(res: Response): Promise<SecretResult> {
  const parsed = MintedSecret.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return refused(CONNECTORS_UNEXPECTED);
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return {
    ok: true,
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
  const gate = await admitConnectorsCall("connectors:webhook", 10, projectId, null);
  if (gate.error !== undefined) return refused(gate.error);
  const trimmed = url.trim();
  if (trimmed.length === 0) return refused("Enter the endpoint URL.");
  if (trimmed.length > 2048) return refused("That URL is too long.");
  const checked = checkKinds(eventKinds);
  if (!checked.ok) return checked;
  const res = await tryFetchWithTimeout(`${gate.base}/internal/v1/connectors/webhook`, {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ url: trimmed, event_kinds: checked.kinds }),
  });
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);
  return readSecret(res);
}

const UpdatedConnection = z.object({ connection: ConnectionSchema });

// Which notices an endpoint receives. A webhook's alone: Slack takes every
// kind, and the service refuses the change for it.
export async function updateWebhookEventsAction(
  projectId: string,
  connectionId: string,
  eventKinds: NotificationKind[] | null,
): Promise<ActionResult> {
  const gate = await admitConnectorsCall("connectors:events", 20, projectId, connectionId);
  if (gate.error !== undefined) return refused(gate.error);
  const checked = checkKinds(eventKinds);
  if (!checked.ok) return checked;
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connectionId)}/events`,
    {
      method: "PUT",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ event_kinds: checked.kinds }),
    },
  );
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);
  const parsed = UpdatedConnection.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return refused(CONNECTORS_UNEXPECTED);
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  return { ok: true };
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
  const gate = await admitConnectorsCall("connectors:rotate", 10, projectId, connectionId);
  if (gate.error !== undefined) return refused(gate.error);
  if (
    !Number.isInteger(keepPreviousForHours) ||
    keepPreviousForHours < 0 ||
    keepPreviousForHours > 24
  ) {
    return refused("Choose how long the current secret keeps working, from 0 to 24 hours.");
  }
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connectionId)}/rotate`,
    {
      method: "POST",
      headers: { ...gate.headers, "content-type": "application/json" },
      body: JSON.stringify({ keep_previous_for_hours: keepPreviousForHours }),
    },
  );
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  if (!res.ok) return refused((await extractProblem(res)).message);
  return readSecret(res);
}

// Sends the same delivery once more, now, under the same id — the receiver's
// dedup on `Telmoni-Delivery-Id` is what makes a resend of a notice it did
// get harmless. Like a test, a refusal that names the connection retires it
// upstream, so the page re-reads either way.
export async function redeliverAction(
  projectId: string,
  connectionId: string,
  deliveryId: string,
): Promise<ActionResult> {
  const gate = await admitConnectorsCall("connectors:redeliver", 10, projectId, connectionId);
  if (gate.error !== undefined) return refused(gate.error);
  if (!z.guid().safeParse(deliveryId).success) {
    return refused("Invalid delivery.");
  }
  const res = await tryFetchWithTimeout(
    `${gate.base}/internal/v1/connectors/${encodeURIComponent(connectionId)}/deliveries/${encodeURIComponent(deliveryId)}/redeliver`,
    { method: "POST", headers: gate.headers },
    SEND_WAIT_MS,
  );
  if (!res) return refused(CONNECTORS_UNAVAILABLE);
  revalidatePath("/(app)/[organization]/[project]/connectors", "page");
  if (!res.ok) return refused((await extractProblem(res)).message);
  const parsed = SendAnswer.safeParse(await res.json().catch(() => null));
  if (!parsed.success) return refused(CONNECTORS_UNEXPECTED);
  if (parsed.data.delivered) return { ok: true };
  return refused(parsed.data.error || "The delivery failed again.");
}
