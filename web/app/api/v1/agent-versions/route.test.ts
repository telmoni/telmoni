import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/versions", () => ({ fetchVersions: vi.fn() }));

import { NextRequest } from "next/server";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { fetchVersions } from "@/lib/server/entities/versions";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const VERSIONS = { days: 14, by: "release", versions: [] };

/** The panel's own read, `query` as the browser sends it. */
function read(query: Record<string, string>) {
  return GET(new NextRequest(`http://localhost:3000/api/v1/agent-versions?${new URLSearchParams(query)}`));
}

describe("GET /api/v1/agent-versions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(fetchVersions).mockResolvedValue({ kind: "ok", value: VERSIONS } as never);
  });

  it("answers the agent's versions, never to be kept by a cache", async () => {
    const res = await read({ project: PROJECT, agent: "triage", by: "release" });
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual(VERSIONS);
    expect(fetchVersions).toHaveBeenCalledWith(PROJECT, {
      agent: "triage",
      by: "release",
      environment: undefined,
      prompt: undefined,
    });
  });

  it.each(["..", ".", "a/b", "100%", " padded ", "agént-日本"])(
    "sends on an agent named %j as it is named",
    async (agent) => {
      expect((await read({ project: PROJECT, agent, by: "model" })).status).toBe(200);
      expect(fetchVersions).toHaveBeenCalledWith(PROJECT, expect.objectContaining({ agent, by: "model" }));
    },
  );

  it("tells the environment with no name from none asked for", async () => {
    await read({ project: PROJECT, agent: "triage", by: "prompt", environment: "", prompt: "a:b" });
    expect(fetchVersions).toHaveBeenCalledWith(PROJECT, {
      agent: "triage",
      by: "prompt",
      environment: "",
      prompt: "a:b",
    });
  });

  it("sends on the days a page reads, and none where they are no count a list reads", async () => {
    await read({ project: PROJECT, agent: "triage", by: "release", days: "30" });
    expect(fetchVersions).toHaveBeenLastCalledWith(PROJECT, expect.objectContaining({ days: 30 }));
    for (const days of ["0", "361", "7.5", "week"]) {
      await read({ project: PROJECT, agent: "triage", by: "release", days });
      expect(vi.mocked(fetchVersions).mock.lastCall?.[1].days, days).toBeUndefined();
    }
  });

  it("refuses a request with no sign-in before reading anything", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await read({ project: PROJECT, agent: "triage", by: "release" })).status).toBe(401);
    expect(fetchVersions).not.toHaveBeenCalled();
  });

  it("holds a reader to their pace", async () => {
    vi.mocked(rateLimitRetryAfter).mockResolvedValueOnce(30);
    expect((await read({ project: PROJECT, agent: "triage", by: "release" })).status).toBe(429);
    expect(fetchVersions).not.toHaveBeenCalled();
  });

  it("sends on nothing but a project's id, an agent and a split it knows", async () => {
    expect((await read({ agent: "triage", by: "release" })).status).toBe(404);
    expect((await read({ project: "../admin", agent: "triage", by: "release" })).status).toBe(404);
    expect((await read({ project: PROJECT, by: "release" })).status).toBe(400);
    expect((await read({ project: PROJECT, agent: "", by: "release" })).status).toBe(400);
    expect((await read({ project: PROJECT, agent: "triage" })).status).toBe(400);
    expect((await read({ project: PROJECT, agent: "triage", by: "tool" })).status).toBe(400);
    expect(fetchVersions).not.toHaveBeenCalled();
  });

  it("answers each way the read can end", async () => {
    const ask = { project: PROJECT, agent: "triage", by: "release" };
    vi.mocked(fetchVersions).mockResolvedValueOnce({ kind: "refused" } as never);
    expect((await read(ask)).status).toBe(400);
    vi.mocked(fetchVersions).mockResolvedValueOnce({ kind: "forbidden" } as never);
    expect((await read(ask)).status).toBe(403);
    vi.mocked(fetchVersions).mockResolvedValueOnce({ kind: "unavailable" } as never);
    expect((await read(ask)).status).toBe(503);
  });
});
