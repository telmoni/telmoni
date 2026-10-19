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
import { fetchFieldInventory } from "./field-inventory";

const PROJECT = "project_1";
const INVENTORY = {
  conventions_commit: "6fd0d763a092db245a54104fc1124031eb3c51d2",
  rows: [
    {
      attribute: "telmoni.run_id",
      on: "every span",
      kept: "the column run_id",
      needs: "any",
      rule: "a UUID, the same on every span of a run",
    },
  ],
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  answer(200, INVENTORY);
});

describe("fetchFieldInventory", () => {
  it("reads the inventory from telemetry's lane, as the project", async () => {
    expect(await fetchFieldInventory(PROJECT)).toEqual(INVENTORY);
    const [url, init] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe("http://server/internal/v1/telemetry/field-inventory");
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
  });

  // Every role on the project reads it, so a refusal is an outage, and the
  // page says the list could not be read rather than show a shorter one.
  it("answers nothing when the lane refuses, or is not there", async () => {
    answer(404, INVENTORY);
    expect(await fetchFieldInventory(PROJECT)).toBeNull();
  });

  it("answers nothing for an answer it cannot read", async () => {
    answer(200, { rows: [] });
    expect(await fetchFieldInventory(PROJECT)).toBeNull();

    answer(200, { conventions_commit: "abc", rows: [{ attribute: "telmoni.run_id" }] });
    expect(await fetchFieldInventory(PROJECT)).toBeNull();
  });

  it("answers nothing when the server cannot be reached", async () => {
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchFieldInventory(PROJECT)).toBeNull();
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchFieldInventory("")).toBeNull();

    identityContext.mockResolvedValue(null);
    expect(await fetchFieldInventory(PROJECT)).toBeNull();
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
