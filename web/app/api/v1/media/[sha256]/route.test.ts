import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({ fetchMedia: vi.fn() }));

import { NextRequest } from "next/server";

import { fetchMedia } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const HASH = "a".repeat(64);

function read(sha256 = HASH, project = PROJECT) {
  return GET(
    new NextRequest(`http://localhost:3000/api/v1/media/${sha256}?project=${encodeURIComponent(project)}`),
    { params: Promise.resolve({ sha256 }) },
  );
}

describe("GET /api/v1/media/[sha256]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
  });

  // ⚠ The browser runs nothing a file holds: never sniffed, sandboxed, kept
  // by no cache, and anything but an image or audio a download.
  it("serves an image inline, and an SVG as a download, each sandboxed and unsniffed", async () => {
    for (const [contentType, disposition] of [
      ["image/png", "inline"],
      ["audio/mpeg", "inline"],
      ["image/svg+xml", "attachment"],
    ] as const) {
      vi.mocked(fetchMedia).mockResolvedValue({ kind: "ok", body: new ArrayBuffer(3), contentType });
      const res = await read();
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toBe(contentType);
      expect(res.headers.get("x-content-type-options")).toBe("nosniff");
      expect(res.headers.get("content-security-policy")).toBe("sandbox");
      expect(res.headers.get("cache-control")).toBe("private, no-store");
      expect(res.headers.get("content-disposition")).toBe(disposition);
    }
    expect(fetchMedia).toHaveBeenCalledWith(PROJECT, HASH);
  });

  it("asks nothing of the server for a name that is no hash or no project", async () => {
    expect((await read("not-a-hash")).status).toBe(404);
    expect((await read(HASH, "nope")).status).toBe(404);
    expect(fetchMedia).not.toHaveBeenCalled();
  });

  it("says what the server said of a file it keeps none of, or will not show", async () => {
    vi.mocked(fetchMedia).mockResolvedValue({ kind: "missing" });
    expect((await read()).status).toBe(404);
    vi.mocked(fetchMedia).mockResolvedValue({ kind: "forbidden" });
    expect((await read()).status).toBe(403);
    vi.mocked(fetchMedia).mockResolvedValue({ kind: "unavailable" });
    expect((await read()).status).toBe(503);
  });
});
