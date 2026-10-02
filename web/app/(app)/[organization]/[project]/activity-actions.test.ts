import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/cache", () => ({ revalidatePath: vi.fn() }));
vi.mock("next/navigation", () => ({
  redirect: vi.fn(() => {
    throw new Error("REDIRECT");
  }),
}));
vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
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
  extractProblem: vi.fn(async () => ({ message: "upstream said no" })),
}));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://notifications.test" },
}));

import { revalidatePath } from "next/cache";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { fetchProject, identityContext, projectHeaders } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { markProjectReadAction } from "./activity-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_abc";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "usr_1",
    email: "ada@example.com",
  } as never);
  vi.mocked(identityContext).mockResolvedValue({
    userId: "usr_1",
    organizationId: "org_1",
    accessToken: "at_1",
  } as never);
  vi.mocked(fetchProject).mockResolvedValue({
    id: PROJECT,
    name: "Platform",
    role: "owner",
  } as never);
  vi.mocked(rateLimit).mockResolvedValue(null as never);
  fetchMock.mockResolvedValue({ ok: true } as never);
});

describe("markProjectReadAction", () => {
  // The project header is what selects the scope. Without it the service answers
  // the ORGANIZATION's feed and refuses anybody but its owner, so a missing
  // project id here would clear a different set than the page counted.
  it("marks read in the project's scope, carrying the project", async () => {
    const res = await markProjectReadAction(PROJECT);

    expect(res).toEqual({ error: null });
    expect(projectHeaders).toHaveBeenCalledWith(expect.anything(), PROJECT);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe("http://notifications.test/internal/notifications/read");
    expect(init).toMatchObject({ method: "POST" });
    expect(
      (init as { headers: Record<string, string> }).headers["x-project-id"],
    ).toBe(PROJECT);
  });

  it("revalidates the project page so the badge and the rows agree", async () => {
    await markProjectReadAction(PROJECT);
    expect(revalidatePath).toHaveBeenCalledWith("/(app)/[organization]/[project]", "page");
  });

  // A Server Action is a public endpoint, so the project it is handed is a claim.
  // No membership row means no role to sign and nothing to mark.
  it("refuses a project the caller has no membership row for", async () => {
    vi.mocked(fetchProject).mockResolvedValue(null as never);
    const res = await markProjectReadAction("project_not_mine");

    expect(res.error).toMatch(/project/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("spends a budget before calling, and stops when it is gone", async () => {
    vi.mocked(rateLimit).mockResolvedValue(1 as never);
    const res = await markProjectReadAction(PROJECT);

    expect(res.error).toMatch(/too many/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("reports an unreachable service without throwing", async () => {
    fetchMock.mockResolvedValue(null as never);
    const res = await markProjectReadAction(PROJECT);

    expect(res.error).toMatch(/unavailable/i);
    expect(revalidatePath).not.toHaveBeenCalled();
  });

  it("surfaces the upstream problem message on a refusal", async () => {
    fetchMock.mockResolvedValue({ ok: false, status: 403 } as never);
    const res = await markProjectReadAction(PROJECT);

    expect(res.error).toBe("upstream said no");
    expect(revalidatePath).not.toHaveBeenCalled();
  });
});
