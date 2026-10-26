import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/cache", () => ({
  refresh: vi.fn(),
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "No incident with that id in this project." })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://server.test",
    SERVICE_SECRET: "secret",
  },
}));
// The real `sessionHeaders`: what reaches the server is the assertion.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
}));

import { refresh } from "next/cache";

import { rateLimit } from "@/lib/api/rate-limit";
import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import { voteIncidentAction } from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
    accessToken: "at_1",
  } as never);
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId: "org_acme",
    role: "member",
    accessToken: "at_1",
  });
});

describe("voteIncidentAction", () => {
  it("posts the person's word on the incident and draws the page again", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 200 }));
    expect(await voteIncidentAction(PROJECT, INCIDENT, "noise")).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://server.test/internal/v1/telemetry/incidents/${INCIDENT}/vote`,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": "org_acme",
          "x-project-id": PROJECT,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ verdict: "noise" }),
      }),
    );
    expect(refresh).toHaveBeenCalledOnce();
  });

  // A Server Action takes whatever is posted to it, and the incident's id
  // becomes part of the lane's path: one that could reach past its own
  // segment, or a word the server does not know, asks nothing.
  it.each<[unknown, unknown]>([
    ["../../rules", "useful"],
    [`${INCIDENT}/resolve`, "useful"],
    ["", "useful"],
    [INCIDENT, "maybe"],
    [INCIDENT, ["useful"]],
  ])("refuses the incident %j voted %j, asking the server nothing", async (incident, verdict) => {
    const res = await voteIncidentAction(PROJECT, incident as string, verdict as string);
    expect(res.error).toMatch(/Couldn't tell which vote this is/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses a project id that could carry another header", async () => {
    const res = await voteIncidentAction("project_1\r\nx-project-id: project_2", INCIDENT, "useful");
    expect(res.error).toMatch(/Couldn't tell which project this is/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks nothing for an expired session, or past the minute's votes", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null);
    expect((await voteIncidentAction(PROJECT, INCIDENT, "useful")).error).toMatch(/session expired/);
    vi.mocked(rateLimit).mockResolvedValueOnce({ retryAfter: 30 } as never);
    expect((await voteIncidentAction(PROJECT, INCIDENT, "useful")).error).toMatch(
      /Too many requests/,
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("says what the service refused, or that it could not be reached, and draws nothing again", async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 404 }));
    expect((await voteIncidentAction(PROJECT, INCIDENT, "useful")).error).toBe(
      "No incident with that id in this project.",
    );
    fetchMock.mockResolvedValueOnce(null);
    expect((await voteIncidentAction(PROJECT, INCIDENT, "useful")).error).toMatch(
      /Alerts are unavailable/,
    );
    expect(refresh).not.toHaveBeenCalled();
  });
});
