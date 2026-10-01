import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/cache", () => ({
  revalidatePath: vi.fn(),
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn((_s: unknown, action: string) => `k:${action}`),
}));
vi.mock("@/lib/server/data", () => ({
  identityContext: vi.fn(),
  fetchProject: vi.fn(),
  projectHeaders: vi.fn((_ctx: unknown, projectId: string) => ({
    authorization: "Bearer at_1",
    "x-organization-id": "org_1",
    "x-project-id": projectId,
  })),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "connection not found", problem: null })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://notifications.test",
  },
}));

import { revalidatePath } from "next/cache";
import { NextResponse } from "next/server";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { fetchProject, identityContext, projectHeaders } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import {
  createWebhookConnectorAction,
  disconnectConnectorAction,
  listDeliveriesAction,
  redeliverAction,
  rotateWebhookSecretAction,
  sendTestMessageAction,
  updateWebhookEventsAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_abc";
const ID = "11111111-1111-4111-8111-111111111111";
const DELIVERY = "44444444-4444-4444-8444-444444444444";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function webhookConnection(overrides: Record<string, unknown> = {}) {
  return {
    id: ID,
    provider: "webhook",
    external_workspace_id: "hooks.example.com",
    external_workspace_name: null,
    channel_id: "3f9a2c1d",
    channel_name: "hooks.example.com",
    status: "active",
    last_error: null,
    last_delivery_at: null,
    created_at: "2026-09-20T12:00:00Z",
    event_kinds: null,
    previous_secret_expires_at: null,
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
  } as never);
  vi.mocked(rateLimit).mockResolvedValue(null);
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId: "org_1",
    role: "owner",
    accessToken: "at_1",
  });
  vi.mocked(fetchProject).mockResolvedValue({ id: PROJECT, name: "Project", role: "owner" });
  fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
});

describe("disconnectConnectorAction", () => {
  // The service refuses a non-owner and this lane cannot decide it: the
  // bearer and the project reach the headers, and the role is auth's to
  // derive from them.
  it("deletes the connection under the project's headers, bearer included", async () => {
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}`,
      expect.objectContaining({
        method: "DELETE",
        headers: expect.objectContaining({ "x-project-id": PROJECT, authorization: "Bearer at_1" }),
      }),
    );
    expect(vi.mocked(projectHeaders)).toHaveBeenCalledWith(expect.anything(), PROJECT);
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  it("treats an already-gone connection as done", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await disconnectConnectorAction(PROJECT, ID)).toEqual({ error: null });
  });

  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 403 }));
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res.error).toBe("connection not found");
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });

  it("refuses a malformed id before any request", async () => {
    const res = await disconnectConnectorAction(PROJECT, "not-a-uuid");
    expect(res.error).toMatch(/invalid connection/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("stops at the ceiling", async () => {
    vi.mocked(rateLimit).mockResolvedValue(new NextResponse(null, { status: 429 }));
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res.error).toMatch(/too many requests/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("stops when there is no session", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res.error).toMatch(/session expired/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("stops when the project cannot be resolved", async () => {
    vi.mocked(fetchProject).mockResolvedValue(null);
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res.error).toMatch(/resolve this project/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reports an unreachable service rather than swallowing it", async () => {
    fetchMock.mockResolvedValue(null);
    const res = await disconnectConnectorAction(PROJECT, ID);
    expect(res.error).toMatch(/unavailable/i);
  });
});

describe("sendTestMessageAction", () => {
  it("posts the test and reports a delivery", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ delivered: true, error: null }), { status: 200 }),
    );
    const res = await sendTestMessageAction(PROJECT, ID);
    expect(res).toEqual({ error: null, delivered: true });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/test`,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ authorization: "Bearer at_1" }),
      }),
      // Longer than the service's own budgets, so it answers first.
      20_000,
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  // The service answers 200 with the vendor's refusal as the payload — the
  // request to us succeeded — so the vendor's words are what the toast says.
  it("surfaces the vendor's answer when the message did not land", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ delivered: false, error: "slack: no_service" }), {
        status: 200,
      }),
    );
    const res = await sendTestMessageAction(PROJECT, ID);
    expect(res).toEqual({ error: "slack: no_service", delivered: false });
    // Re-read either way: a dead-install answer retired the row upstream.
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 409 }));
    const res = await sendTestMessageAction(PROJECT, ID);
    expect(res.error).toBe("connection not found");
  });
});

describe("createWebhookConnectorAction", () => {
  const minted = () =>
    new Response(
      JSON.stringify({
        connection: { id: ID, channel_name: "hooks.example.com" },
        signing_secret: "whsec_abc",
      }),
      { status: 201, headers: { "content-type": "application/json" } },
    );

  // The URL goes to the service as typed, under the project headers; the
  // service is the validator. What comes back is shown once, so the action
  // hands the secret straight to the dialog and keeps nothing.
  it("posts the URL as JSON and returns the secret once", async () => {
    fetchMock.mockResolvedValue(minted());
    const res = await createWebhookConnectorAction(
      PROJECT,
      "  https://hooks.example.com/x ",
      null,
    );
    expect(res).toEqual({ error: null, signingSecret: "whsec_abc", host: "hooks.example.com" });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://notifications.test/internal/connectors/webhook",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "x-project-id": PROJECT,
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ url: "https://hooks.example.com/x", event_kinds: null }),
      }),
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  it("sends a chosen list of kinds as it was chosen", async () => {
    fetchMock.mockResolvedValue(minted());
    await createWebhookConnectorAction(PROJECT, "https://hooks.example.com/x", [
      "member_added",
      "connector_connected",
    ]);
    expect(fetchMock).toHaveBeenCalledWith(
      "http://notifications.test/internal/connectors/webhook",
      expect.objectContaining({
        body: JSON.stringify({
          url: "https://hooks.example.com/x",
          event_kinds: ["member_added", "connector_connected"],
        }),
      }),
    );
  });

  // An empty list is a webhook that receives nothing: the service refuses
  // it, and so does the action, before spending the request.
  it("refuses an empty selection before any request", async () => {
    const res = await createWebhookConnectorAction(PROJECT, "https://hooks.example.com/x", []);
    expect(res.error).toMatch(/at least one event/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a kind outside the generated vocabulary", async () => {
    const res = await createWebhookConnectorAction(PROJECT, "https://hooks.example.com/x", [
      "member_removed" as never,
    ]);
    expect(res.error).toMatch(/unknown event/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("returns the service's refusal as the message and stores nothing", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 400 }));
    const res = await createWebhookConnectorAction(PROJECT, "http://hooks.example.com/x", null);
    expect(res).toEqual({ error: "connection not found" });
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });

  it("refuses an empty URL before any request", async () => {
    const res = await createWebhookConnectorAction(PROJECT, "   ", null);
    expect(res.error).toMatch(/enter the endpoint/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not treat a shape it cannot read as a secret", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ ok: true }), { status: 201 }));
    const res = await createWebhookConnectorAction(PROJECT, "https://hooks.example.com/x", null);
    expect(res.error).toMatch(/unexpected response/i);
    expect(res.signingSecret).toBeUndefined();
  });

  it("stops at the ceiling", async () => {
    vi.mocked(rateLimit).mockResolvedValue(new NextResponse(null, { status: 429 }));
    const res = await createWebhookConnectorAction(PROJECT, "https://hooks.example.com/x", null);
    expect(res.error).toMatch(/too many requests/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("updateWebhookEventsAction", () => {
  it("puts the chosen kinds and re-reads the page", async () => {
    fetchMock.mockResolvedValue(
      json({ connection: webhookConnection({ event_kinds: ["member_added"] }) }),
    );
    const res = await updateWebhookEventsAction(PROJECT, ID, ["member_added"]);
    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/events`,
      expect.objectContaining({
        method: "PUT",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ event_kinds: ["member_added"] }),
      }),
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  it("sends null for every kind", async () => {
    fetchMock.mockResolvedValue(json({ connection: webhookConnection() }));
    await updateWebhookEventsAction(PROJECT, ID, null);
    expect(fetchMock).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ body: JSON.stringify({ event_kinds: null }) }),
    );
  });

  it("refuses an empty selection before any request", async () => {
    const res = await updateWebhookEventsAction(PROJECT, ID, []);
    expect(res.error).toMatch(/at least one event/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a kind outside the generated vocabulary", async () => {
    const res = await updateWebhookEventsAction(PROJECT, ID, ["nope" as never]);
    expect(res.error).toMatch(/unknown event/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a malformed id before any request", async () => {
    const res = await updateWebhookEventsAction(PROJECT, "not-a-uuid", null);
    expect(res.error).toMatch(/invalid connection/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 403 }));
    const res = await updateWebhookEventsAction(PROJECT, ID, null);
    expect(res.error).toBe("connection not found");
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });

  it("reports a shape it cannot read", async () => {
    fetchMock.mockResolvedValue(json({ ok: true }));
    const res = await updateWebhookEventsAction(PROJECT, ID, null);
    expect(res.error).toMatch(/unexpected response/i);
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });
});

describe("rotateWebhookSecretAction", () => {
  const rotated = () =>
    json({
      connection: { id: ID, channel_name: "hooks.example.com" },
      signing_secret: "whsec_new",
    });

  it("posts the rotation with its overlap and returns the new secret once", async () => {
    fetchMock.mockResolvedValue(rotated());
    const res = await rotateWebhookSecretAction(PROJECT, ID, 24);
    expect(res).toEqual({ error: null, signingSecret: "whsec_new", host: "hooks.example.com" });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/rotate`,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ keep_previous_for_hours: 24 }),
      }),
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  // Zero is the hard cut, and it still goes as a body: an absent one would
  // leave the overlap to the service's default.
  it("sends zero for a hard cut", async () => {
    fetchMock.mockResolvedValue(rotated());
    await rotateWebhookSecretAction(PROJECT, ID, 0);
    expect(fetchMock).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ body: JSON.stringify({ keep_previous_for_hours: 0 }) }),
    );
  });

  it.each([-1, 25, 1.5, Number.NaN])("refuses an overlap of %s hours", async (hours) => {
    const res = await rotateWebhookSecretAction(PROJECT, ID, hours);
    expect(res.error).toMatch(/0 to 24 hours/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 400 }));
    expect(await rotateWebhookSecretAction(PROJECT, ID, 1)).toEqual({
      error: "connection not found",
    });
  });

  it("refuses a malformed id before any request", async () => {
    const res = await rotateWebhookSecretAction(PROJECT, "not-a-uuid", 24);
    expect(res.error).toMatch(/invalid connection/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("listDeliveriesAction", () => {
  const page = {
    deliveries: [
      {
        id: DELIVERY,
        kind: "member_added",
        subject: "Ada joined Project",
        body: "Ada accepted an invitation.",
        status: "failed",
        next_attempt_at: "2026-09-21T12:00:00Z",
        last_error: "HTTP 500",
        created_at: "2026-09-20T12:00:00Z",
        updated_at: "2026-09-21T12:00:00Z",
        attempts: [
          {
            id: "55555555-5555-4555-8555-555555555555",
            trigger: "scheduled",
            outcome: "failed",
            status_code: 500,
            duration_ms: 120,
            error: "HTTP 500",
            created_at: "2026-09-20T12:00:01Z",
          },
        ],
      },
    ],
    next_before: DELIVERY,
  };

  it("reads the first page under the project's headers", async () => {
    fetchMock.mockResolvedValue(json(page));
    const res = await listDeliveriesAction(PROJECT, ID, null);
    expect(res).toEqual({ error: null, page });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/deliveries?limit=25`,
      expect.objectContaining({
        method: "GET",
        headers: expect.objectContaining({ authorization: "Bearer at_1" }),
      }),
    );
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });

  it("pages back from the cursor", async () => {
    fetchMock.mockResolvedValue(json({ deliveries: [], next_before: null }));
    await listDeliveriesAction(PROJECT, ID, DELIVERY);
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/deliveries?limit=25&before=${DELIVERY}`,
      expect.anything(),
    );
  });

  // A member reads the log too: the role is the service's to check, so the
  // action asks the same way whatever the project role.
  it("reads for a member as for an owner", async () => {
    vi.mocked(fetchProject).mockResolvedValue({ id: PROJECT, name: "Project", role: "member" });
    fetchMock.mockResolvedValue(json(page));
    expect((await listDeliveriesAction(PROJECT, ID, null)).error).toBeNull();
  });

  it("refuses a malformed cursor before any request", async () => {
    const res = await listDeliveriesAction(PROJECT, ID, "not-a-uuid");
    expect(res.error).toMatch(/invalid delivery/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a malformed connection id before any request", async () => {
    const res = await listDeliveriesAction(PROJECT, "not-a-uuid", null);
    expect(res.error).toMatch(/invalid connection/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 404 }));
    expect(await listDeliveriesAction(PROJECT, ID, null)).toEqual({
      error: "connection not found",
    });
  });

  it("reports a shape it cannot read", async () => {
    fetchMock.mockResolvedValue(json({ deliveries: [{ id: DELIVERY }], next_before: null }));
    const res = await listDeliveriesAction(PROJECT, ID, null);
    expect(res.error).toMatch(/unexpected response/i);
    expect(res.page).toBeUndefined();
  });

  it("reports an unreachable service", async () => {
    fetchMock.mockResolvedValue(null);
    expect((await listDeliveriesAction(PROJECT, ID, null)).error).toMatch(/unavailable/i);
  });
});

describe("redeliverAction", () => {
  const url = `http://notifications.test/internal/connectors/${ID}/deliveries/${DELIVERY}/redeliver`;

  it("posts the resend and reports a delivery", async () => {
    fetchMock.mockResolvedValue(json({ delivered: true, error: null }));
    const res = await redeliverAction(PROJECT, ID, DELIVERY);
    expect(res).toEqual({ error: null, delivered: true });
    expect(fetchMock).toHaveBeenCalledWith(
      url,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ authorization: "Bearer at_1" }),
      }),
      20_000,
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  it("surfaces the receiver's answer when the resend did not land", async () => {
    fetchMock.mockResolvedValue(json({ delivered: false, error: "HTTP 503" }));
    const res = await redeliverAction(PROJECT, ID, DELIVERY);
    expect(res).toEqual({ error: "HTTP 503", delivered: false });
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith(`/${PROJECT}/connectors`);
  });

  // 409 is a stopped connection or an attempt already in flight; the
  // problem's own sentence says which.
  it("returns the service's refusal as the message", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 409 }));
    expect(await redeliverAction(PROJECT, ID, DELIVERY)).toEqual({
      error: "connection not found",
    });
    expect(vi.mocked(revalidatePath)).not.toHaveBeenCalled();
  });

  it("refuses a malformed delivery id before any request", async () => {
    const res = await redeliverAction(PROJECT, ID, "not-a-uuid");
    expect(res.error).toMatch(/invalid delivery/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a malformed connection id before any request", async () => {
    const res = await redeliverAction(PROJECT, "not-a-uuid", DELIVERY);
    expect(res.error).toMatch(/invalid connection/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reports a shape it cannot read", async () => {
    fetchMock.mockResolvedValue(json({ ok: true }));
    const res = await redeliverAction(PROJECT, ID, DELIVERY);
    expect(res.error).toMatch(/unexpected response/i);
    expect(res.delivered).toBeUndefined();
  });
});
