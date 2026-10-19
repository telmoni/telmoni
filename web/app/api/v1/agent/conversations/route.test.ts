import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/agent", () => ({ listAgentConversations: vi.fn() }));

import { NextRequest } from "next/server";

import { listAgentConversations } from "@/lib/server/entities/agent";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function get() {
  return GET(new NextRequest(`http://localhost:3000/api/v1/agent/conversations?project=${PROJECT}`));
}

describe("GET /api/v1/agent/conversations", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ accessToken: "at_1" } as never);
  });

  it("lists the person's past conversations on the project", async () => {
    const conversations = [{ id: "c1", title: "Why did the run fail?" }];
    vi.mocked(listAgentConversations).mockResolvedValue(conversations as never);
    const res = await get();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({ conversations });
    expect(listAgentConversations).toHaveBeenCalledWith(PROJECT);
  });

  it("says the agent is unavailable when the history cannot be read", async () => {
    vi.mocked(listAgentConversations).mockResolvedValue(null as never);
    const res = await get();
    expect(res.status).toBe(503);
    expect(await res.json()).toEqual({
      error: "The agent is unavailable right now — try again shortly.",
    });
  });

  it("refuses an ended session, reading nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await get()).status).toBe(401);
    expect(listAgentConversations).not.toHaveBeenCalled();
  });
});
