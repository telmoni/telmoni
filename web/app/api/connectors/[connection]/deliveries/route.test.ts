import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn((_s: unknown, call: string) => `k:${call}`),
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
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://notifications.test" } }));

import { NextRequest } from "next/server";

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { fetchProject, identityContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_abc";
const ID = "11111111-1111-4111-8111-111111111111";
const DELIVERY = "44444444-4444-4444-8444-444444444444";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

function get(connection = ID, before?: string) {
  const query = new URLSearchParams({ project: PROJECT });
  if (before !== undefined) query.set("before", before);
  return GET(
    new NextRequest(`http://localhost:3000/api/connectors/${connection}/deliveries?${query}`),
    { params: Promise.resolve({ connection }) },
  );
}

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
  vi.mocked(fetchProject).mockResolvedValue({
    id: PROJECT,
    slug: "project",
    name: "Project",
    role: "owner",
  });
  fetchMock.mockResolvedValue(json(page));
});

describe("GET /api/connectors/[connection]/deliveries", () => {
  it("reads the first page under the project's headers, for this session alone", async () => {
    const res = await get();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual(page);
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/deliveries?limit=25`,
      expect.objectContaining({
        method: "GET",
        headers: expect.objectContaining({ authorization: "Bearer at_1" }),
      }),
    );
    expect(rateLimit).toHaveBeenCalledWith("k:connectors:deliveries", {
      limit: 60,
      windowMs: 60_000,
    });
  });

  it("pages back from the cursor", async () => {
    fetchMock.mockResolvedValue(json({ deliveries: [], next_before: null }));
    await get(ID, DELIVERY);
    expect(fetchMock).toHaveBeenCalledWith(
      `http://notifications.test/internal/connectors/${ID}/deliveries?limit=25&before=${DELIVERY}`,
      expect.anything(),
    );
  });

  // A member reads the log too: the role is the service's to check, so the
  // handler asks the same way whatever the project role.
  it("reads for a member as for an owner", async () => {
    vi.mocked(fetchProject).mockResolvedValue({
      id: PROJECT,
      slug: "project",
      name: "Project",
      role: "member",
    });
    expect((await get()).status).toBe(200);
  });

  it("refuses a malformed cursor before any request", async () => {
    const res = await get(ID, "not-a-uuid");
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ error: "Invalid delivery." });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a malformed connection id before any request", async () => {
    const res = await get("not-a-uuid");
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: "Invalid connection." });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses an ended session, a caller past the limit and an unknown project, asking nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null as never);
    expect((await get()).status).toBe(401);

    vi.mocked(rateLimit).mockResolvedValueOnce(30 as never);
    expect((await get()).status).toBe(429);

    vi.mocked(fetchProject).mockResolvedValueOnce(null);
    const res = await get();
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({
      error: "Couldn't resolve this project. Reload and try again.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("returns the service's refusal as the message, under its status", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 404 }));
    const res = await get();
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: "connection not found" });
  });

  it("answers the service's own failure as a bad gateway", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 500 }));
    vi.mocked(extractProblem).mockResolvedValueOnce({
      message: "Internal Server Error",
      problem: null,
    });
    const res = await get();
    expect(res.status).toBe(502);
    expect(await res.json()).toEqual({ error: "Internal Server Error" });
  });

  it("reports a shape it cannot read", async () => {
    fetchMock.mockResolvedValue(json({ deliveries: [{ id: DELIVERY }], next_before: null }));
    const res = await get();
    expect(res.status).toBe(502);
    expect((await res.json()).error).toMatch(/unexpected response/i);
  });

  it("reports an unreachable service", async () => {
    fetchMock.mockResolvedValue(null);
    const res = await get();
    expect(res.status).toBe(503);
    expect((await res.json()).error).toMatch(/unavailable/i);
  });
});
