import { beforeEach, describe, expect, it, vi } from "vitest";

const mockCreate = vi.hoisted(() => vi.fn());
const mockSubscribeCmd = vi.hoisted(() => vi.fn(async () => 1));
const mockUnsubscribeCmd = vi.hoisted(() => vi.fn(async () => 0));
const handlers = vi.hoisted<Record<string, ((...args: never[]) => void)[]>>(() => ({}));

vi.mock("@/lib/redis", () => ({
  createRedisSubscriber: () => mockCreate(),
}));

// Mocked rather than spied: `load()` re-imports the module under test after
// `vi.resetModules()`, which hands it a FRESH logger instance that a spy on
// the statically-imported one would never see.
const mockWarn = vi.hoisted(() => vi.fn());
vi.mock("@/lib/logger", () => ({
  logger: { warn: mockWarn, error: vi.fn(), info: vi.fn(), debug: vi.fn() },
}));

function fakeClient() {
  return {
    status: "ready",
    subscribe: (...channels: string[]) => mockSubscribeCmd(...(channels as [])),
    unsubscribe: (...channels: string[]) => mockUnsubscribeCmd(...(channels as [])),
    on: (event: string, cb: (...args: never[]) => void) => {
      (handlers[event] ??= []).push(cb);
    },
  };
}

function emit(channel: string, message: string) {
  for (const cb of handlers.message ?? []) {
    (cb as (c: string, m: string) => void)(channel, message);
  }
}

function ready() {
  for (const cb of handlers.ready ?? []) (cb as () => void)();
}

async function load() {
  return (await import("./subscriber")).subscribe;
}

describe("events subscriber", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    for (const key of Object.keys(handlers)) delete handlers[key];
    mockCreate.mockReturnValue(fakeClient());
  });

  it("opens one connection and sends one SUBSCRIBE per channel, however many listeners", async () => {
    const subscribe = await load();
    subscribe(["bfev:user:a", "bfev:organization:x"], vi.fn());
    subscribe(["bfev:user:b", "bfev:organization:x"], vi.fn());

    expect(mockCreate).toHaveBeenCalledTimes(1);
    expect(mockSubscribeCmd).toHaveBeenCalledTimes(2);
    expect(mockSubscribeCmd).toHaveBeenNthCalledWith(1, "bfev:user:a", "bfev:organization:x");
    expect(mockSubscribeCmd).toHaveBeenNthCalledWith(2, "bfev:user:b");
  });

  it("fans a message out to every listener on its channel and to nobody else", async () => {
    const subscribe = await load();
    const onX1 = vi.fn();
    const onX2 = vi.fn();
    const onY = vi.fn();
    subscribe(["bfev:organization:x"], onX1);
    subscribe(["bfev:organization:x"], onX2);
    subscribe(["bfev:organization:y"], onY);

    emit("bfev:organization:x", '{"type":"invite:resolved"}');

    expect(onX1).toHaveBeenCalledWith("bfev:organization:x", '{"type":"invite:resolved"}');
    expect(onX2).toHaveBeenCalledWith("bfev:organization:x", '{"type":"invite:resolved"}');
    expect(onY).not.toHaveBeenCalled();
  });

  it("sends UNSUBSCRIBE for a channel only when its last listener leaves", async () => {
    const subscribe = await load();
    const stopFirst = subscribe(["bfev:organization:x", "bfev:user:a"], vi.fn());
    const stopSecond = subscribe(["bfev:organization:x"], vi.fn());

    stopFirst();
    expect(mockUnsubscribeCmd).toHaveBeenCalledTimes(1);
    expect(mockUnsubscribeCmd).toHaveBeenCalledWith("bfev:user:a");

    stopSecond();
    expect(mockUnsubscribeCmd).toHaveBeenCalledTimes(2);
    expect(mockUnsubscribeCmd).toHaveBeenLastCalledWith("bfev:organization:x");

    expect(() => emit("bfev:organization:x", "{}")).not.toThrow();
  });

  it("stopping twice is harmless", async () => {
    const subscribe = await load();
    const stop = subscribe(["bfev:organization:x"], vi.fn());
    stop();
    stop();
    expect(mockUnsubscribeCmd).toHaveBeenCalledTimes(1);
  });

  it("a listener that throws does not stop the others", async () => {
    const subscribe = await load();
    const warn = mockWarn;
    const bad = vi.fn(() => {
      throw new Error("boom");
    });
    const good = vi.fn();
    subscribe(["bfev:organization:x"], bad);
    subscribe(["bfev:organization:x"], good);

    emit("bfev:organization:x", "{}");

    expect(good).toHaveBeenCalled();
    expect(warn).toHaveBeenCalled();
  });

  it("holds a SUBSCRIBE until the connection is ready, then sends the whole table", async () => {
    const client = fakeClient();
    client.status = "connecting";
    mockCreate.mockReturnValue(client);
    const subscribe = await load();
    subscribe(["bfev:user:a"], vi.fn());
    subscribe(["bfev:organization:x"], vi.fn());
    expect(mockSubscribeCmd).not.toHaveBeenCalled();

    client.status = "ready";
    ready();
    expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);
    expect(mockSubscribeCmd).toHaveBeenCalledWith("bfev:user:a", "bfev:organization:x");
  });

  it("re-sends every live channel on a reconnect, and none that were left", async () => {
    const subscribe = await load();
    const stop = subscribe(["bfev:user:a"], vi.fn());
    subscribe(["bfev:organization:x"], vi.fn());
    stop();
    mockSubscribeCmd.mockClear();

    ready();
    expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);
    expect(mockSubscribeCmd).toHaveBeenCalledWith("bfev:organization:x");
  });

  it("sends nothing on a ready with nobody listening", async () => {
    const subscribe = await load();
    const stop = subscribe(["bfev:user:a"], vi.fn());
    stop();
    mockSubscribeCmd.mockClear();

    ready();
    expect(mockSubscribeCmd).not.toHaveBeenCalled();
  });

  it("ends no subscription on a connection that is down, and does not bring it back", async () => {
    const client = fakeClient();
    mockCreate.mockReturnValue(client);
    const subscribe = await load();
    const stop = subscribe(["bfev:user:a"], vi.fn());
    expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);

    client.status = "reconnecting";
    stop();
    expect(mockUnsubscribeCmd).not.toHaveBeenCalled();

    client.status = "ready";
    ready();
    expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);
  });

  it("is a no-op without Redis", async () => {
    mockCreate.mockReturnValue(null);
    const subscribe = await load();
    const stop = subscribe(["bfev:organization:x"], vi.fn());
    expect(mockSubscribeCmd).not.toHaveBeenCalled();
    expect(() => stop()).not.toThrow();
  });

  // ⚠ **A SUBSCRIBE that fails while the connection is UP is the silent one.**
  // Nothing downstream can tell a channel it never joined from a channel
  // nobody published to: the stream stays open and the keepalive keeps
  // ticking. A reconnect re-sends the whole table; an in-place failure used to
  // be logged and dropped.
  it("retries a subscribe that failed on a live connection", async () => {
    vi.useFakeTimers();
    try {
      const subscribe = await load();
      mockSubscribeCmd.mockRejectedValueOnce(new Error("ERR something"));

      subscribe(["bfev:user:a"], vi.fn());
      await vi.advanceTimersByTimeAsync(0);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);
      expect(mockWarn).toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(1_000);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(2);
      expect(mockSubscribeCmd).toHaveBeenNthCalledWith(2, "bfev:user:a");
    } finally {
      vi.useRealTimers();
    }
  });

  it("re-sends the whole table, so a channel added after the failure is not left out", async () => {
    vi.useFakeTimers();
    try {
      const subscribe = await load();
      mockSubscribeCmd.mockRejectedValueOnce(new Error("ERR something"));

      subscribe(["bfev:user:a"], vi.fn());
      await vi.advanceTimersByTimeAsync(0);
      subscribe(["bfev:organization:x"], vi.fn());

      await vi.advanceTimersByTimeAsync(1_000);
      expect(mockSubscribeCmd).toHaveBeenLastCalledWith(
        "bfev:user:a",
        "bfev:organization:x",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("answers a burst of failures with one retry, not one each", async () => {
    vi.useFakeTimers();
    try {
      const subscribe = await load();
      mockSubscribeCmd.mockRejectedValue(new Error("ERR something"));

      subscribe(["bfev:user:a"], vi.fn());
      subscribe(["bfev:user:b"], vi.fn());
      subscribe(["bfev:user:c"], vi.fn());
      await vi.advanceTimersByTimeAsync(0);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(3);

      await vi.advanceTimersByTimeAsync(1_000);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(4);
    } finally {
      mockSubscribeCmd.mockReset();
      mockSubscribeCmd.mockImplementation(async () => 1);
      vi.useRealTimers();
    }
  });

  it("gives up quietly once the last listener has gone", async () => {
    vi.useFakeTimers();
    try {
      const subscribe = await load();
      mockSubscribeCmd.mockRejectedValueOnce(new Error("ERR something"));

      const stop = subscribe(["bfev:user:a"], vi.fn());
      await vi.advanceTimersByTimeAsync(0);
      stop();

      await vi.advanceTimersByTimeAsync(1_000);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("leaves a retry to the reconnect when the connection went down too", async () => {
    vi.useFakeTimers();
    try {
      const client = fakeClient();
      mockCreate.mockReturnValue(client);
      const subscribe = await load();
      mockSubscribeCmd.mockRejectedValueOnce(new Error("ERR something"));

      subscribe(["bfev:user:a"], vi.fn());
      await vi.advanceTimersByTimeAsync(0);
      client.status = "reconnecting";

      await vi.advanceTimersByTimeAsync(1_000);
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(1);

      // `ready` is what covers it, and it sends the same table.
      client.status = "ready";
      ready();
      expect(mockSubscribeCmd).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
