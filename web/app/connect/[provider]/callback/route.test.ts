// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  tryFetchWithTimeout,
  getSession,
  unsealConnect,
  identityContext,
  fetchProjectAnywhere,
  projectHeaders,
  accountHeaders,
} =
  vi.hoisted(() => ({
    tryFetchWithTimeout: vi.fn(),
    getSession: vi.fn(),
    unsealConnect: vi.fn(),
    identityContext: vi.fn(),
    fetchProjectAnywhere: vi.fn(),
    projectHeaders: vi.fn((_ctx: unknown, projectId: string) => ({
      authorization: "Bearer at_1",
      "x-project-id": projectId,
    })),
    accountHeaders: vi.fn(),
  }));

vi.mock("@/lib/api/fetch", () => ({ tryFetchWithTimeout }));
vi.mock("@/lib/auth/session", () => ({
  CONNECT_COOKIE: "telmoni_connect",
  getSession,
  unsealConnect,
}));
vi.mock("@/lib/server/data", () => ({ identityContext, fetchProjectAnywhere, projectHeaders }));
vi.mock("@/lib/server/entities/identity-context", () => ({ accountHeaders }));
vi.mock("@/lib/env", () => ({
  env: {
    AUTH_URL: "https://app.example",
    SERVER_URL: "http://notifications:8086",
  },
}));

import { NextRequest } from "next/server";

import { GET } from "./route";

const PROJECT = "project_abc";
const ORGANIZATION = "org_home";
// By id, as the cookie sealed them: the console redirects it to the slugs the
// project and its organization go by now.
const PAGE = `https://app.example/${ORGANIZATION}/${PROJECT}/connectors`;

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
  unsealConnect.mockReset().mockResolvedValue({
    purpose: "install",
    state: "st8",
    organizationId: ORGANIZATION,
    projectId: PROJECT,
    provider: "slack",
  });
  accountHeaders.mockReset().mockResolvedValue({
    authorization: "Bearer at_1",
    "x-organization-id": "org_elsewhere",
  });
  // The request stands in `org_elsewhere` — whichever organization a tab
  // opened last — while the project is `org_home`'s.
  identityContext
    .mockReset()
    .mockResolvedValue({ userId: "usr_1", organizationId: "org_elsewhere", accessToken: "at_1" });
  fetchProjectAnywhere.mockReset().mockResolvedValue({
    id: PROJECT,
    slug: "platform",
    name: "Platform",
    role: "owner",
    organizationId: ORGANIZATION,
    organizationSlug: "acme",
  });
  tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status: 201 }));
});

describe("GET /connect/[provider]/callback", () => {
  it("posts the code and state to the service under the cookie's project, clears the cookie and lands on the page", async () => {
    const res = await callback();
    expect(res.status).toBe(307);
    expect(res.headers.get("location")).toBe(`${PAGE}?connected=slack`);
    expect(tryFetchWithTimeout).toHaveBeenCalledWith(
      "http://notifications:8086/internal/v1/connectors/slack/callback",
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
    expect(fetchProjectAnywhere).toHaveBeenCalledWith(PROJECT);
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  // ⚠ This path names no organization, so the request stands wherever the
  // cookie last pointed. The project's own organization is the one to name.
  it("acts in the project's organization, not the one the request stands in", async () => {
    await callback();
    expect(projectHeaders).toHaveBeenCalledWith(
      expect.objectContaining({ organizationId: ORGANIZATION, accessToken: "at_1" }),
      PROJECT,
    );
  });

  it("lands a project the caller can no longer read back on the page, and asks the service nothing", async () => {
    fetchProjectAnywhere.mockResolvedValue(null);
    const res = await callback();
    expect(res.headers.get("location")).toBe(`${PAGE}?error=project`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
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

  it("refuses a provider the console does not offer", async () => {
    const res = await callback("?code=c0de&state=st8", "teams");
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

// The same return address, for a person linking their own Slack account from
// their account settings: no project, and back to that page either way.
describe("GET /connect/[provider]/callback, linking a person's own account", () => {
  const ACCOUNT = "https://app.example/settings/account";

  beforeEach(() => {
    unsealConnect.mockResolvedValue({ purpose: "identity", state: "st8", provider: "slack" });
  });

  it("posts the code and state as the person's own, clears the cookie and lands on their settings", async () => {
    const res = await callback();
    expect(res.headers.get("location")).toBe(`${ACCOUNT}?slack=linked#slack`);
    expect(tryFetchWithTimeout).toHaveBeenCalledWith(
      "http://notifications:8086/internal/v1/notifications/slack/identities/callback",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "content-type": "application/json",
        }),
        body: JSON.stringify({ code: "c0de", state: "st8" }),
      }),
    );
    expect(fetchProjectAnywhere).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  it("says so when the Slack account is linked to somebody else", async () => {
    tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status: 409 }));
    const res = await callback();
    expect(res.headers.get("location")).toBe(`${ACCOUNT}?slack=elsewhere#slack`);
  });

  // ⚠ The same edge CSRF check as an install's.
  it.each([
    ["?code=c0de&state=forged", "callback"],
    ["?state=st8", "callback"],
    ["?error=access_denied&state=st8", "denied"],
  ])("refuses %s before touching the service, as ?slack=%s", async (query, outcome) => {
    const res = await callback(query);
    expect(res.headers.get("location")).toBe(`${ACCOUNT}?slack=${outcome}#slack`);
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
    expect(res.cookies.get("telmoni_connect")?.value).toBe("");
  });

  it("sends a lapsed session through login, keeping the cookie", async () => {
    getSession.mockResolvedValue(null);
    const res = await callback();
    expect(res.headers.get("location")).toContain("https://app.example/auth/login?returnTo=");
    expect(res.cookies.get("telmoni_connect")).toBeUndefined();
    expect(tryFetchWithTimeout).not.toHaveBeenCalled();
  });

  it("lands an unreachable service, or a refusal, back on the settings", async () => {
    tryFetchWithTimeout.mockResolvedValue(null);
    expect((await callback()).headers.get("location")).toBe(
      `${ACCOUNT}?slack=unavailable#slack`,
    );
    tryFetchWithTimeout.mockResolvedValue(new Response("{}", { status: 400 }));
    expect((await callback()).headers.get("location")).toBe(`${ACCOUNT}?slack=callback#slack`);
  });
});
