import { beforeEach, describe, expect, it, vi } from "vitest";

import { errorOf } from "@/lib/action-result";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "a project's cap on alerts is at most the plan's, 10" })),
}));
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://telemetry.test", SERVICE_SECRET: "secret" } }));
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("next/cache", () => ({ revalidatePath: vi.fn() }));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import { setProjectCapsAction } from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const ORGANIZATION = "org_acme";
const PROJECT = "project_7bQx2mNv9BcK4dLp";
const CAPS = {
  spans: 200_000,
  content_bytes: null,
  alerts: 10,
  monitors: null,
  objectives: null,
  alert_deliveries_per_day: null,
};

function as(role: "owner" | "admin" | "member") {
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId: ORGANIZATION,
    role,
    accessToken: "at_1",
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "admin@example.test",
    accessToken: "at_1",
  } as never);
  as("admin");
});

describe("setProjectCapsAction", () => {
  it("sets the project's caps whole on its scope, as an admin", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify(CAPS), { status: 200 }));
    expect(await setProjectCapsAction(ORGANIZATION, PROJECT, CAPS)).toEqual({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://telemetry.test/internal/v1/telemetry/limits",
      expect.objectContaining({
        method: "PUT",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "x-project-id": PROJECT,
        }),
        body: JSON.stringify(CAPS),
      }),
    );
  });

  it("refuses a member, a project id that is none, and a cap that is no whole number", async () => {
    as("member");
    expect(errorOf(await setProjectCapsAction(ORGANIZATION, PROJECT, CAPS))).toMatch(/owner or an admin/);
    as("admin");
    expect(errorOf(await setProjectCapsAction(ORGANIZATION, "../x", CAPS))).toMatch(/could not be found/);
    expect(
      errorOf(await setProjectCapsAction(ORGANIZATION, PROJECT, { ...CAPS, alerts: -1 })),
    ).toMatch(/whole number/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("says the lane's refusal", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 400 }));
    expect(errorOf(await setProjectCapsAction(ORGANIZATION, PROJECT, { ...CAPS, alerts: 11 }))).toBe(
      "a project's cap on alerts is at most the plan's, 10",
    );
  });
});
