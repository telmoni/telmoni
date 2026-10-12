import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/agent", () => ({ openAgentConversation: vi.fn() }));

import { NextRequest } from "next/server";

import { openAgentConversation } from "@/lib/server/entities/agent";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const ID = "0f8fad5b-d9cb-469f-a165-70867728950e";

function get(id = ID) {
  return GET(
    new NextRequest(`http://localhost:3000/api/agent/conversations/${id}?project=${PROJECT}`),
    { params: Promise.resolve({ id }) },
  );
}

describe("GET /api/agent/conversations/[id]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ accessToken: "at_1" } as never);
  });

  it("opens the conversation, whole", async () => {
    const conversation = { id: ID, messages: [] };
    vi.mocked(openAgentConversation).mockResolvedValue({ kind: "ok", conversation } as never);
    const res = await get();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({ conversation });
    expect(openAgentConversation).toHaveBeenCalledWith(PROJECT, ID);
  });

  it("says a conversation that is gone is gone, and one that cannot be read is unavailable", async () => {
    vi.mocked(openAgentConversation).mockResolvedValueOnce({ kind: "not-found" } as never);
    const gone = await get();
    expect(gone.status).toBe(404);
    expect(await gone.json()).toEqual({ error: "That conversation no longer exists." });

    vi.mocked(openAgentConversation).mockResolvedValueOnce({ kind: "unavailable" } as never);
    expect((await get()).status).toBe(503);
  });

  it("refuses an id that is not one, asking nothing", async () => {
    const res = await get("../../status");
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ error: "Unknown conversation." });
    expect(openAgentConversation).not.toHaveBeenCalled();
  });
});
