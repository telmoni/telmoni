import { beforeEach, describe, expect, it, vi } from "vitest";

const { notFound, getServerContext, fetchWithTimeout } = vi.hoisted(() => ({
  notFound: vi.fn(() => {
    throw new Error("NEXT_NOT_FOUND");
  }),
  getServerContext: vi.fn(),
  fetchWithTimeout: vi.fn(),
}));

vi.mock("next/navigation", () => ({ notFound }));
vi.mock("./entities/organization", () => ({ getServerContext }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("@/lib/env", () => ({
  env: { SERVICE_SECRET: "s", SERVER_URL: "http://auth" },
}));
vi.mock("@/lib/logger", () => ({ logger: { warn: vi.fn() } }));

import { Flag } from "@/lib/flags";

import { featureOff, requireFeature } from "./flags";

const ctx = (flags: Record<string, boolean>) => ({
  person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
  organizations: [
    { organizationId: "org_1", name: "Ada", ownerEmail: "ada@example.test", role: "owner" },
  ],
  activeOrganizationId: "org_1",
  memberships: [],
  incomingInvites: [],
  flags,
});

beforeEach(() => {
  notFound.mockClear();
  getServerContext.mockReset();
  fetchWithTimeout.mockReset();
});

describe("requireFeature — the page half of a switched-off row", () => {
  it("is a 404 when every listed flag is off", async () => {
    getServerContext.mockResolvedValue(ctx({ api_tokens: false }));
    await expect(requireFeature(Flag.ApiTokens)).rejects.toThrow("NEXT_NOT_FOUND");
    expect(notFound).toHaveBeenCalledOnce();
  });

  it("keeps a shared page while ANY of its flags is on", async () => {
    getServerContext.mockResolvedValue(ctx({ api_tokens: false, public_api: true }));
    await requireFeature(Flag.ApiTokens, Flag.PublicApi);
    expect(notFound).not.toHaveBeenCalled();
  });

  it("reads an absent key as on — absence is not a decision", async () => {
    getServerContext.mockResolvedValue(ctx({}));
    await requireFeature(Flag.Members);
    expect(notFound).not.toHaveBeenCalled();
  });

  it("does nothing with no context, so the page renders its own outage card", async () => {
    getServerContext.mockResolvedValue(null);
    await requireFeature(Flag.ApiTokens);
    expect(notFound).not.toHaveBeenCalled();
  });
});

describe("featureOff — the Server Action half", () => {
  it("answers the catalog's sentence for a present false and null for an absent key", async () => {
    getServerContext.mockResolvedValue(ctx({ api_tokens: false }));
    expect(await featureOff(Flag.ApiTokens)).toBe("API keys are switched off right now.");
    expect(await featureOff(Flag.Members)).toBeNull();
  });

  it("fails CLOSED with no context — and says so, not that a switch was thrown", async () => {
    getServerContext.mockResolvedValue(null);
    expect(await featureOff(Flag.Members)).toBe(
      "We couldn't verify your organization just now. Try again shortly.",
    );
  });
});
