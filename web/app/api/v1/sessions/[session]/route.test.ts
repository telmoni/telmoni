import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({ fetchSession: vi.fn() }));

import { NextRequest, NextResponse } from "next/server";

import { rateLimit } from "@/lib/api/rate-limit";
import { fetchSession } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SESSION = "chat/42 b";
const DETAIL = { session_id: SESSION, traces: [], pinned: ["plan"], days: 7, limit: 100, more: false };

/** The session page's own read, the segment as Next hands it over: decoded. */
function read(query = `?project=${PROJECT}&pinned=plan`, session = SESSION) {
  return GET(new NextRequest(`http://localhost:3000/api/v1/sessions/${encodeURIComponent(session)}${query}`), {
    params: Promise.resolve({ session }),
  });
}

describe("GET /api/v1/sessions/[session]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(fetchSession).mockResolvedValue({ kind: "ok", session: DETAIL } as never);
  });

  it("answers the session with the spans pinned, never to be kept by a cache", async () => {
    const res = await read(`?project=${PROJECT}&days=30&limit=500&pinned=plan&pinned=%20plan%20&pinned=answer`);
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual(DETAIL);
    expect(fetchSession).toHaveBeenCalledWith(PROJECT, SESSION, { days: 30, limit: 500 }, ["plan", "answer"]);
  });

  it("refuses a request with no sign-in before reading anything", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await read()).status).toBe(401);
    expect(fetchSession).not.toHaveBeenCalled();
  });

  it("holds a reader to their pace", async () => {
    vi.mocked(rateLimit).mockResolvedValueOnce(NextResponse.json({}, { status: 429 }));
    expect((await read()).status).toBe(429);
    expect(fetchSession).not.toHaveBeenCalled();
  });

  it("sends on nothing but a project's id, a session's id and up to eight names", async () => {
    expect((await read("?pinned=plan")).status).toBe(404);
    expect((await read("?project=../admin&pinned=plan")).status).toBe(404);
    expect((await read(`?project=${PROJECT}`, "   ")).status).toBe(404);
    expect((await read(`?project=${PROJECT}`, "s".repeat(201))).status).toBe(404);
    const nine = Array.from({ length: 9 }, (_, i) => `pinned=s${i}`).join("&");
    expect((await read(`?project=${PROJECT}&${nine}`)).status).toBe(400);
    expect((await read(`?project=${PROJECT}&pinned=${"n".repeat(146)}`)).status).toBe(400);
    expect(fetchSession).not.toHaveBeenCalled();
  });

  it("counts a session's id and a pinned name in characters, as the server does", async () => {
    const wide = "😀".repeat(200);
    expect((await read(`?project=${PROJECT}&pinned=${"😀".repeat(145)}`, wide)).status).toBe(200);
    expect(fetchSession).toHaveBeenCalledWith(PROJECT, wide, {}, ["😀".repeat(145)]);
    expect((await read(`?project=${PROJECT}`, `${wide}😀`)).status).toBe(404);
    expect((await read(`?project=${PROJECT}&pinned=${"😀".repeat(146)}`)).status).toBe(400);
    expect(fetchSession).toHaveBeenCalledTimes(1);
  });

  it("drops a window the server would refuse rather than refusing", async () => {
    expect((await read(`?project=${PROJECT}&days=0&limit=x`)).status).toBe(200);
    expect(fetchSession).toHaveBeenCalledWith(PROJECT, SESSION, {}, []);
  });

  it("answers each way the read can end", async () => {
    vi.mocked(fetchSession).mockResolvedValueOnce({ kind: "missing" } as never);
    expect((await read()).status).toBe(404);
    vi.mocked(fetchSession).mockResolvedValueOnce({ kind: "forbidden" } as never);
    expect((await read()).status).toBe(403);
    vi.mocked(fetchSession).mockResolvedValueOnce({ kind: "unavailable" } as never);
    expect((await read()).status).toBe(503);
  });
});
