// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const { tryFetchWithTimeout, getSession, unsealConnect, identityContext, fetchProject, projectHeaders } =
  vi.hoisted(() => ({
    tryFetchWithTimeout: vi.fn(),
    getSession: vi.fn(),
    unsealConnect: vi.fn(),
    identityContext: vi.fn(),
    fetchProject: vi.fn(),
    projectHeaders: vi.fn((_ctx: unknown, projectId: string) => ({
      authorization: "Bearer at_1",
      "x-project-id": projectId,
    })),
  }));

vi.mock("@/lib/api/fetch", () => ({ tryFetchWithTimeout }));
vi.mock("@/lib/auth/session", () => ({
  CONNECT_COOKIE: "telmoni_connect",
  getSession,
  unsealConnect,
}));
vi.mock("@/lib/server/data", () => ({ identityContext, fetchProject, projectHeaders }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://notifications:8086",
  },
}));

import { NextRequest } from "next/server";

import { GET } from "./route";

const PROJECT = "project_abc";
const PAGE = `https://app.example/${PROJECT}/connectors`;

function callback(query = "?code=c0de&state=st8", provider = "slack", cookie = "sealed") {
  return GET(
    new NextRequest(`https://app.example/connect/${provider}/callback${query}`, {
      headers: cookie ? { cookie: `telmoni_connect=${cookie}` } : {},
    }),
    { params: Promise.resolve({ provider }) },
  );
}

beforeEach(() => {
  tryFetchWithTimeout.mockReset();
  projectHeaders.mockClear();
  getSession.mockReset().mockResolvedValue({ userId: "usr_1" });
  unsealConnect.mockReset().mockResolvedValue({ state: "st8", projectId: PROJECT, provider: "slack" });
  identityContext
    .mockReset()
    .mockResolvedValue({ userId: "usr_1", organizationId: "org_1", accessToken: "at_1" });
  fetchProject.mockReset().mockResolvedValue({ id: PROJECT, name: "Project", role: "owner" });
  tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status: 201 }));
});

describe("GET /connect/[provider]/callback", () => {
  it("posts the code and state to the service under the cookie's project, clears the cookie and lands on the page", async () => {
    const res = await callback();
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(`${PAGE}?connected=slack`);
    expect(tryFetchWithTimeout).toHaveBeenCalledWith(
      "http://notifications:8086/internal/connectors/slack/callback",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          "x-project-id": PROJECT,
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ code: "c0de", state: "st8" }),
      }),
    );
    // The project came from the COOKIE — the query names none.
    expect(fetchProject).toHaveBeenCalledWith(PROJECT);
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  // ⚠ The edge CSRF check: a state that is not the cookie's is refused
  // before any request, and the handshake is over.
  it("refuses a state that is not the cookie's before touching the service", async () => {
    const res = await callback("?code=c0de&state=forged");
    expect(res.headers.get("location")).toBe(`${PAGE}?error=callback`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  it("lands on the console with no cookie — there is no project to land on", async () => {
    unsealConnect.mockResolvedValue(null);
    const res = await callback("?code=c0de&state=st8", "slack", "");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a cookie sealed for the other provider", async () => {
    const res = await callback("?code=c0de&state=st8", "discord");
    expect(res.headers.get("location")).toBe("https://app.example/console");
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("reports a cancelled consent without a service call", async () => {
    const res = await callback("?error=access_denied&state=st8");
    expect(res.headers.get("location")).toBe(`${PAGE}?error=denied`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  it("sends a lapsed session through login with the code and state intact, keeping the cookie", async () => {
    getSession.mockResolvedValue(null);
    const res = await callback();
    expect(res.headers.get("location")).toBe(
      `https://app.example/auth/login?returnTo=${encodeURIComponent("/connect/slack/callback?code=c0de&state=st8")}`,
    );
    expect(res.cookies.get("telmoni_connect")).toBeUndefined();
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it.each([
    [403, "forbidden"],
    [400, "callback"],
  ])("lands a %i from the service back on the page as ?error=%s", async (status, error) => {
    tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status }));
    const res = await callback();
    expect(res.headers.get("location")).toBe(`${PAGE}?error=${error}`);
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  it("lands an unreachable service back on the page", async () => {
    tryFetchWithTimeout.mockResolvedValue(null);
    const res = await callback();
    expect(res.headers.get("location")).toBe(`${PAGE}?error=unavailable`);
  });

  it("requires a code", async () => {
    const res = await callback("?state=st8");
    expect(res.headers.get("location")).toBe(`${PAGE}?error=callback`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });
});
