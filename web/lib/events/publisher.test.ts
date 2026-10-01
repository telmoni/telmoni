import { beforeEach, describe, expect, it, vi } from "vitest";

const mockPublish = vi.fn();
const mockGetRedis = vi.fn();

vi.mock("@/lib/redis", () => ({
  getRedis: () => mockGetRedis(),
}));

import { organizationChannel, publishEvent, publishToAll, userChannel } from "./publisher";

describe("events publisher", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetRedis.mockReturnValue({
      publish: mockPublish,
    });
  });

  it("reaches one channel however the address was typed", () => {
    expect(userChannel(" Ada.Lovelace@Example.COM ")).toBe(
      userChannel("ada.lovelace@example.com"),
    );
  });

  // The whole point: `PUBSUB CHANNELS` must not answer "here is everyone".
  it("carries no part of the address into the channel name", () => {
    const channel = userChannel("ada.lovelace@example.com");
    expect(channel).toMatch(/^bfev:user:[0-9a-f]{32}$/);
    expect(channel).not.toContain("@");
    for (const fragment of ["ada", "lovelace", "example.com", "example"]) {
      expect(channel).not.toContain(fragment);
    }
  });

  it("gives two addresses two channels", () => {
    expect(userChannel("a@example.com")).not.toBe(userChannel("b@example.com"));
  });

  // A bare digest of an address is confirmable offline against a list of
  // addresses. Keyed, the same list is worthless without the secret.
  it("is keyed, so the name cannot be derived from the address alone", () => {
    const before = userChannel("a@example.com");
    const original = process.env.AUTH_SECRET;
    process.env.AUTH_SECRET = "another-secret-that-is-32-bytes!!!!";
    try {
      expect(userChannel("a@example.com")).not.toBe(before);
    } finally {
      process.env.AUTH_SECRET = original;
    }
    expect(userChannel("a@example.com")).toBe(before);
  });

  it("leaves the organization channel legible, because an id is not PII", () => {
    expect(organizationChannel("acc_1")).toBe("bfev:organization:acc_1");
  });

  it("normalizes organizationChannel to trimmed string", () => {
    expect(organizationChannel(" project_12345 ")).toBe("bfev:organization:project_12345");
  });

  it("publishes serialized event to the channel", async () => {
    mockPublish.mockResolvedValue(1);

    const ok = await publishEvent(userChannel("test@example.com"), {
      type: "invite:resolved",
      data: { inviteId: "inv_123" },
    });

    expect(ok).toBe(true);
    expect(mockPublish).toHaveBeenCalledWith(
      userChannel("test@example.com"),
      JSON.stringify({
        type: "invite:resolved",
        data: { inviteId: "inv_123" },
      }),
    );
  });

  it("returns false if Redis is unavailable", async () => {
    mockGetRedis.mockReturnValue(null);

    const ok = await publishEvent(userChannel("test@example.com"), {
      type: "invite:resolved",
      data: { inviteId: "inv_123" },
    });

    expect(ok).toBe(false);
    expect(mockPublish).not.toHaveBeenCalled();
  });

  it("publishToAll tells each named channel once and skips the unnamed", async () => {
    mockPublish.mockResolvedValue(1);

    await publishToAll(
      [userChannel("a@example.com"), null, undefined, "", userChannel("A@example.com"), organizationChannel("acc_1")],
      { type: "invite:resolved", data: { inviteId: "inv_1" } },
    );

    expect(mockPublish).toHaveBeenCalledTimes(2);
    expect(mockPublish).toHaveBeenNthCalledWith(1, userChannel("a@example.com"), expect.any(String));
    expect(mockPublish).toHaveBeenNthCalledWith(2, organizationChannel("acc_1"), expect.any(String));
  });

  it("fails gracefully and returns false if publish throws", async () => {
    mockPublish.mockRejectedValue(new Error("Connection lost"));

    const ok = await publishEvent(userChannel("test@example.com"), {
      type: "invite:resolved",
      data: { inviteId: "inv_123" },
    });

    expect(ok).toBe(false);
  });
});
