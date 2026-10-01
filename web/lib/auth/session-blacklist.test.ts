import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockRedis = {
  status: "ready",
  get: vi.fn(),
  set: vi.fn(),
  publish: vi.fn(),
};

vi.mock("@/lib/redis", () => ({
  getRedis: vi.fn(() => mockRedis),
}));

import { getRedis } from "@/lib/redis";
import {
  blacklistSession,
  isSessionBlacklisted,
  blacklistSizeForTest,
  clearBlacklistForTest,
} from "./session-blacklist";

const mockedGetRedis = vi.mocked(getRedis);

describe("session-blacklist", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clearBlacklistForTest();
    process.env.REDIS_URL = "redis://localhost:6379";
    mockRedis.status = "ready";
    mockRedis.get.mockResolvedValue(null);
    mockRedis.set.mockResolvedValue("OK");
    mockRedis.publish.mockResolvedValue(1);
  });

  afterEach(() => {
    clearBlacklistForTest();
  });

  it("adds session to blacklist and writes to Redis", async () => {
    await blacklistSession("sess_abc", 3600);

    expect(mockRedis.set).toHaveBeenCalledWith(
      "bfsb:sess_abc",
      "1",
      "EX",
      3600,
    );
    expect(await isSessionBlacklisted("sess_abc")).toBe(true);
  });

  it("announces the revocation on the session's channel, after the key is written", async () => {
    await blacklistSession("sess_abc", 3600);

    expect(mockRedis.publish).toHaveBeenCalledWith("bfev:session:sess_abc", "revoked");
    const [setOrder] = mockRedis.set.mock.invocationCallOrder;
    const [publishOrder] = mockRedis.publish.mock.invocationCallOrder;
    expect(setOrder).toBeLessThan(publishOrder!);
  });

  it("a failed announcement leaves the blacklist entry standing", async () => {
    mockRedis.publish.mockRejectedValueOnce(new Error("ECONNRESET"));

    await expect(blacklistSession("sess_abc", 3600)).resolves.toBeUndefined();
    expect(mockRedis.set).toHaveBeenCalledOnce();
    expect(await isSessionBlacklisted("sess_abc")).toBe(true);
  });

  it("returns false for non-blacklisted session", async () => {
    expect(await isSessionBlacklisted("sess_nonexistent")).toBe(false);
  });

  it("checks Redis if not in in-memory map", async () => {
    mockRedis.get.mockResolvedValueOnce("1");

    expect(await isSessionBlacklisted("sess_remote")).toBe(true);
    expect(mockRedis.get).toHaveBeenCalledWith("bfsb:sess_remote");
  });

  it("fails open when Redis errors", async () => {
    mockRedis.get.mockRejectedValueOnce(new Error("ECONNREFUSED"));

    expect(await isSessionBlacklisted("sess_error")).toBe(false);
  });

  it("falls back to in-memory when Redis is null or not ready", async () => {
    mockedGetRedis.mockReturnValueOnce(null);
    await blacklistSession("sess_in_mem", 60);

    mockedGetRedis.mockReturnValueOnce(null);
    expect(await isSessionBlacklisted("sess_in_mem")).toBe(true);
  });

  it("prunes expired items from in-memory cache", async () => {
    await blacklistSession("sess_expired", 0);

    await new Promise((r) => setTimeout(r, 5));
    expect(await isSessionBlacklisted("sess_expired")).toBe(false);
  });

  // ⚠ **A revoked session is never presented again, so nothing ever asks
  // about it a second time.** Entries used to leave this map only on that
  // second question, which meant they never left at all.
  it("does not keep an entry for a session nobody will ever ask about again", async () => {
    vi.useFakeTimers();
    try {
      for (let i = 0; i < 50; i += 1) {
        await blacklistSession(`sess_${i}`, 60);
      }
      expect(blacklistSizeForTest()).toBe(50);

      vi.advanceTimersByTime(61_000);
      await blacklistSession("sess_later", 60);

      expect(
        blacklistSizeForTest(),
        "the expired fifty are still held, keyed by a session id nobody has",
      ).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps an entry that has not expired, sweep or no sweep", async () => {
    vi.useFakeTimers();
    try {
      await blacklistSession("sess_long", 3600);
      vi.advanceTimersByTime(61_000);
      await blacklistSession("sess_other", 60);

      expect(await isSessionBlacklisted("sess_long")).toBe(true);
      expect(blacklistSizeForTest()).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
