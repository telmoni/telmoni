import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const envMock = vi.hoisted(
  () => ({}) as { ANALYTICS_INGEST_URL?: string; ANALYTICS_WRITE_KEY?: string },
);
const mockLogger = vi.hoisted(() => ({
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  debug: vi.fn(),
  child: vi.fn().mockReturnThis(),
}));
vi.mock("./logger", () => ({ logger: mockLogger, requestLogger: vi.fn(() => mockLogger) }));
vi.mock("./env", () => ({ env: envMock }));

import { track } from "./analytics";

describe("track()", () => {
  const fetchMock = vi.fn(() =>
    Promise.resolve(new Response(null, { status: 200 })),
  );

  beforeEach(() => {
    fetchMock.mockClear();
    vi.stubGlobal("fetch", fetchMock);
    envMock.ANALYTICS_INGEST_URL = undefined;
    envMock.ANALYTICS_WRITE_KEY = undefined;
  });
  afterEach(() => vi.unstubAllGlobals());

  it("sends to the provider only on explicit consent, and logs either way", async () => {
    envMock.ANALYTICS_INGEST_URL = "https://provider.test/capture";
    envMock.ANALYTICS_WRITE_KEY = "phc_test";

    track("onboarding.signup_completed", { userId: "usr_1" });
    await Promise.resolve();
    expect(fetchMock, "an unflagged call reached the provider").not.toHaveBeenCalled();
    expect(mockLogger.info).toHaveBeenCalledWith(
      expect.objectContaining({ analytics: true, userId: "usr_1" }),
      "onboarding.signup_completed",
    );

    track("onboarding.signup_completed", { userId: "usr_1" }, { consented: false });
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();

    track("onboarding.signup_completed", { userId: "usr_1" }, { consented: true });
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("never throws and never posts when the provider is unconfigured", async () => {
    expect(() => track("activation.topup_started", { ok: true })).not.toThrow();
    await Promise.resolve();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("posts a PostHog-shaped capture with the userId as distinct_id", async () => {
    envMock.ANALYTICS_INGEST_URL = "https://ph.example/capture/";
    envMock.ANALYTICS_WRITE_KEY = "phc_key";

    track("onboarding.signup_completed", { userId: "user_42" }, { consented: true });
    await Promise.resolve();
    await Promise.resolve();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ];
    expect(url).toBe("https://ph.example/capture/");
    const body = JSON.parse(init.body as string);
    expect(body).toMatchObject({
      api_key: "phc_key",
      event: "onboarding.signup_completed",
      distinct_id: "user_42",
      properties: { userId: "user_42" },
    });
  });

  it("falls back to an anonymous distinct_id when no userId is present", async () => {
    envMock.ANALYTICS_INGEST_URL = "https://ph.example/capture/";
    envMock.ANALYTICS_WRITE_KEY = "phc_key";

    track("activation.topup_started", { amount: 100 }, { consented: true });
    await Promise.resolve();
    await Promise.resolve();

    const [, init] = fetchMock.mock.calls[0] as unknown as [
      string,
      RequestInit,
    ];
    expect(JSON.parse(init.body as string).distinct_id).toBe("anonymous");
  });

  it("swallows a provider rejection — analytics never breaks the caller", async () => {
    envMock.ANALYTICS_INGEST_URL = "https://ph.example/capture/";
    envMock.ANALYTICS_WRITE_KEY = "phc_key";
    fetchMock.mockRejectedValueOnce(new Error("network down"));

    expect(() =>
      track("onboarding.signup_completed", { userId: "u" }, { consented: true }),
    ).not.toThrow();
    await Promise.resolve();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
