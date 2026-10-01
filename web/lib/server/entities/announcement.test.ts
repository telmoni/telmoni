import { beforeEach, describe, expect, it, vi } from "vitest";

const mockGet = vi.fn();
vi.mock("@/lib/redis", () => ({
  getRedis: () => ({
    get: mockGet,
  }),
}));

import { fetchProjectAnnouncement, REDIS_ANNOUNCEMENT_KEY } from "./announcement";

describe("fetchProjectAnnouncement", () => {
  const originalEnv = process.env;

  beforeEach(() => {
    vi.clearAllMocks();
    process.env = { ...originalEnv };
    delete process.env.TELMONI_ANNOUNCEMENT;
    delete process.env.NEXT_PUBLIC_TELMONI_ANNOUNCEMENT;
  });

  it("returns null when neither Redis nor environment variable is set", async () => {
    mockGet.mockResolvedValue(null);
    const result = await fetchProjectAnnouncement();
    expect(result).toBeNull();
  });

  it("reads structured announcement from Redis broadcast key", async () => {
    mockGet.mockResolvedValue(
      JSON.stringify({
        message: "Maintenance scheduled for 02:00 UTC",
        linkText: "Status page",
        href: "https://status.telmoni.com",
      }),
    );

    const result = await fetchProjectAnnouncement();
    expect(mockGet).toHaveBeenCalledWith(REDIS_ANNOUNCEMENT_KEY);
    expect(result).toEqual({
      message: "Maintenance scheduled for 02:00 UTC",
      linkText: "Status page",
      href: "https://status.telmoni.com",
    });
  });

  it("reads plain string announcement from Redis broadcast key", async () => {
    mockGet.mockResolvedValue("Important platform update from the Telmoni Project.");

    const result = await fetchProjectAnnouncement();
    expect(result).toEqual({
      message: "Important platform update from the Telmoni Project.",
    });
  });

  it("falls back to TELMONI_ANNOUNCEMENT environment variable when Redis is empty", async () => {
    mockGet.mockResolvedValue(null);
    process.env.TELMONI_ANNOUNCEMENT = "Telmoni Cloud v2 is now live!";

    const result = await fetchProjectAnnouncement();
    expect(result).toEqual({
      message: "Telmoni Cloud v2 is now live!",
    });
  });
});
