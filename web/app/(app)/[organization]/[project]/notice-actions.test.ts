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
  extractProblem: vi.fn(async () => ({ message: "You are not on this project." })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
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
import { getServerContext } from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { markProjectReadAction } from "./notice-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The project the overview rendered, and handed to the button.
const PROJECT = "project_7bQx2mNv9BcK4dLp";
// The newest notice the page drew, as the server spells a `created_at`.
const DRAWN = "2026-10-17T12:00:00.123456Z";

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

describe("markProjectReadAction", () => {
  it("marks the feed of the project the page rendered read, and draws the page again", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ marked: 2 }), { status: 200 }));
    expect(await markProjectReadAction(PROJECT, DRAWN)).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/notifications/read",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": "org_acme",
          "x-project-id": PROJECT,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ before: DRAWN }),
      }),
    );
    expect(refresh).toHaveBeenCalledOnce();
  });

  // The time is posted too: one the page could not have drawn asks nothing.
  it.each(["yesterday", "", "2026-10-17", "2026-10-17T12:00:00"])(
    "refuses %j as the newest notice drawn, asking the server nothing",
    async (before) => {
      expect((await markProjectReadAction(PROJECT, before)).error).toMatch(/what the page showed/);
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  // A Server Action takes whatever is posted to it, and the id becomes a
  // header of the request: one that could carry another header, or reach
  // past its own segment, asks nothing.
  it.each<unknown>(["project_1\r\nx-project-id: project_2", "../organization", "", ["project_1"]])(
    "refuses %j, asking the server nothing",
    async (posted) => {
      const res = await markProjectReadAction(posted as string, DRAWN);
      expect(res.error).toMatch(/Couldn't tell which project this is/);
      expect(fetchMock).not.toHaveBeenCalled();
      expect(refresh).not.toHaveBeenCalled();
    },
  );

  it("tells a tab that missed a move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as Awaited<ReturnType<typeof getServerContext>>);
    const res = await markProjectReadAction(PROJECT, DRAWN);
    expect(res.error).toMatch(/URL was changed, or you're no longer in it/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks nothing for an expired session, or past the minute's marks", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null);
    expect((await markProjectReadAction(PROJECT, DRAWN)).error).toMatch(/session expired/);
    vi.mocked(rateLimit).mockResolvedValueOnce({ retryAfter: 30 } as never);
    expect((await markProjectReadAction(PROJECT, DRAWN)).error).toMatch(/Too many requests/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // The service decides who is on the project: its refusal is the answer.
  it("says what the service refused, or that it could not be reached, and draws nothing again", async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 403 }));
    expect((await markProjectReadAction(PROJECT, DRAWN)).error).toBe("You are not on this project.");
    fetchMock.mockResolvedValueOnce(null);
    expect((await markProjectReadAction(PROJECT, DRAWN)).error).toMatch(/Notifications are unavailable/);
    expect(refresh).not.toHaveBeenCalled();
  });
});
