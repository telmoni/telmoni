import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, identityContext } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  identityContext: vi.fn(),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("./identity-context", () => ({
  identityContext,
  organizationHeaders: (ctx: { organizationId: string }) => ({
    authorization: "Bearer at_1",
    "x-organization-id": ctx.organizationId,
  }),
}));
import { fetchOrganizationHealth } from "./organization-health";

const OPEN = {
  id: "5d4f5c68-0000-4000-8000-000000000001",
  rule_id: "7a1c0e52-0000-4000-8000-000000000002",
  rule_name: "Slow calls",
  status: "open",
  severity: "alert",
  value: 6200,
  opened_at: "2026-10-27T03:00:00Z",
  acknowledged_at: null,
  acknowledged_by: null,
  resolved_at: null,
  resolved_by: null,
  reopened: 0,
  told_to: [],
  runs: [],
  project: { id: "project_1", slug: "web", name: "Web" },
};

function answer(status: number, body?: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as Response;
}

beforeEach(() => {
  fetchWithTimeout.mockReset();
  identityContext.mockReset();
  identityContext.mockResolvedValue({ organizationId: "org_1", accessToken: "at_1" });
});

describe("fetchOrganizationHealth", () => {
  it("reads the acting organization's open incidents, each with its project", async () => {
    fetchWithTimeout.mockResolvedValue(answer(200, { incidents: [OPEN] }));
    expect(await fetchOrganizationHealth()).toEqual({ kind: "ok", incidents: [OPEN] });
    expect(fetchWithTimeout).toHaveBeenCalledWith(
      "http://server/internal/v1/telemetry/organizations/org_1/health",
      expect.objectContaining({
        headers: expect.objectContaining({ "x-organization-id": "org_1" }),
        cache: "no-store",
      }),
    );
  });

  it("reads a 403 as forbidden, and any other failure as an outage", async () => {
    fetchWithTimeout.mockResolvedValueOnce(answer(403));
    expect(await fetchOrganizationHealth()).toEqual({ kind: "forbidden" });
    fetchWithTimeout.mockResolvedValueOnce(answer(500));
    expect(await fetchOrganizationHealth()).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockResolvedValueOnce(answer(200, { incidents: [{ ...OPEN, project: null }] }));
    expect(await fetchOrganizationHealth()).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValueOnce(new Error("connect ECONNREFUSED"));
    expect(await fetchOrganizationHealth()).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a signed-in person in an organization", async () => {
    identityContext.mockResolvedValue(null);
    expect(await fetchOrganizationHealth()).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
