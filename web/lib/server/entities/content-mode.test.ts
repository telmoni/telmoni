import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, identityContext } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  identityContext: vi.fn(),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("./identity-context", () => ({
  identityContext,
  projectHeaders: (_ctx: unknown, projectId: string) => ({
    authorization: "Bearer at_1",
    "x-project-id": projectId,
  }),
}));
import { fetchContentMode } from "./content-mode";

const PROJECT = "project_1";

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  answer(200, { content_mode: "off", offered: ["off"] });
});

describe("fetchContentMode", () => {
  it("reads the project's mode, and the modes its switch offers, from telemetry's lane", async () => {
    expect(await fetchContentMode(PROJECT)).toEqual({ content_mode: "off", offered: ["off"] });
    const [url, init] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe("http://server/internal/telemetry/content-mode");
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
  });

  // Every role on the project reads its mode, so a refusal is an outage, and
  // the page says the mode could not be read rather than show a wrong one.
  it("answers nothing when the lane refuses, or is not there", async () => {
    // A body it could read, so that only the status turns it away.
    answer(404, { content_mode: "off", offered: ["off"] });
    expect(await fetchContentMode(PROJECT)).toBeNull();
  });

  it("answers nothing for an answer it cannot read", async () => {
    answer(200, { mode: "off" });
    expect(await fetchContentMode(PROJECT)).toBeNull();

    answer(200, { content_mode: "sometimes", offered: ["off"] });
    expect(await fetchContentMode(PROJECT)).toBeNull();
  });

  it("answers nothing when the server cannot be reached", async () => {
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchContentMode(PROJECT)).toBeNull();
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchContentMode("")).toBeNull();

    identityContext.mockResolvedValue(null);
    expect(await fetchContentMode(PROJECT)).toBeNull();
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
