import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "problem" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
    SERVICE_SECRET: "secret",
  },
}));
// The real `organizationHeaders`: what reaches auth is the assertion under test.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Error(`REDIRECT:${to}`);
  },
}));
const mockRevalidate = vi.fn();
vi.mock("next/cache", () => ({
  revalidatePath: (...a: unknown[]) => mockRevalidate(...a),
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import { renameOrganizationAction } from "./rename-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The organization the settings page rendered, and handed to the form.
const ORGANIZATION = "org_acme";
const SWITCHED =
  "You switched organizations in another tab. Reload this page to act on the one you're viewing.";

function standingIn(organizationId: string) {
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId,
    role: "owner",
    accessToken: "at_1",
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "owner@example.test",
    accessToken: "at_1",
  } as never);
  standingIn(ORGANIZATION);
});

describe("renameOrganizationAction", () => {
  it("renames the organization the page rendered, under the session's bearer", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await renameOrganizationAction(ORGANIZATION, "  Acme Robotics  ")).toEqual({
      error: null,
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/name",
      expect.objectContaining({
        method: "PUT",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ name: "Acme Robotics" }),
      }),
    );
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
  });

  // ⚠ Another tab switched the shared cookie after this page rendered, so
  // `/me` answers that organization at the click. Sent on, the name typed into
  // A's box would have renamed B — an owner of both would never know which.
  it("refuses without asking auth when another tab moved the console elsewhere", async () => {
    standingIn("org_elsewhere");
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: SWITCHED,
    });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("refuses without asking auth when it cannot tell where the caller stands", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: "Couldn't resolve your organization right now. Try again in a moment.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
