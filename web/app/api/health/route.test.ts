import { describe, it, expect } from "vitest";
import { GET } from "./route";

describe("GET /api/health", () => {
  it("returns 200 with status ok", async () => {
    const res  = await GET();
    const data = await res.json() as { status: string };
    expect(res.status).toBe(200);
    expect(data.status).toBe("ok");
  });

  // ⚠ **One key, and adding a second is the mistake this guards.** The obvious
  // improvement to a health route is to report what it depends on, and this
  // door is public and unauthenticated: a `redis` field would tell a stranger
  // both that we run one and the moment the shared limiter degraded to a
  // per-replica one — which is the moment it is cheapest to spend. Dependency
  // state belongs in the logs, which reach an operator and nobody else.
  it("says nothing else — the probe is public and answers strangers", async () => {
    const res  = await GET();
    expect(Object.keys(await res.json())).toEqual(["status"]);
  });
});
