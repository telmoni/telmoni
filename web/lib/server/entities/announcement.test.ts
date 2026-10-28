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
  });

  // A `NEXT_PUBLIC_` name is inlined when the image is built, and the
  // published image is built with none: only the runtime name is read.
  it("reads no NEXT_PUBLIC_ name for the banner", async () => {
    mockGet.mockResolvedValue(null);
    process.env.NEXT_PUBLIC_TELMONI_ANNOUNCEMENT = "frozen at build";
    expect(await fetchProjectAnnouncement()).toBeNull();
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

  // A hand-typed value whose link is no string reaches no `<Link>`: the
  // broadcast is passed over for the environment's, as one with no message is.
  it("passes over a shape that is no announcement, rather than drawing it", async () => {
    mockGet.mockResolvedValue(JSON.stringify({ message: "Read this", linkText: "Here", href: 5 }));
    process.env.TELMONI_ANNOUNCEMENT = JSON.stringify({ message: "From the environment" });
    expect(await fetchProjectAnnouncement()).toEqual({ message: "From the environment" });
    mockGet.mockResolvedValue(JSON.stringify({ href: "/somewhere" }));
    delete process.env.TELMONI_ANNOUNCEMENT;
    expect(await fetchProjectAnnouncement()).toBeNull();
  });
});
