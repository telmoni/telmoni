import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/agent", () => ({ agentStatus: vi.fn() }));

import { NextRequest } from "next/server";

import { rateLimit } from "@/lib/api/rate-limit";
import { agentStatus } from "@/lib/server/entities/agent";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function get(project = PROJECT) {
  return GET(
    new NextRequest(`http://localhost:3000/api/v1/agent/status?project=${encodeURIComponent(project)}`),
  );
}

describe("GET /api/v1/agent/status", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ accessToken: "at_1" } as never);
    vi.mocked(rateLimit).mockResolvedValue(null as never);
    vi.mocked(agentStatus).mockResolvedValue("ready" as never);
  });

  it("answers whether the agent can answer on the project, for this session alone", async () => {
    const res = await get();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({ status: "ready" });
    expect(agentStatus).toHaveBeenCalledWith(PROJECT);
  });

  it("refuses an ended session, a caller past the limit and a malformed project, asking nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null as never);
    expect((await get()).status).toBe(401);

    vi.mocked(rateLimit).mockResolvedValueOnce(30 as never);
    expect((await get()).status).toBe(429);

    const res = await get("../members");
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: "Unknown project." });
    expect(agentStatus).not.toHaveBeenCalled();
  });
});
