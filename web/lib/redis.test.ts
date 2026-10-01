import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ctor = vi.hoisted(() => vi.fn());
vi.mock("ioredis", () => ({
  default: class {
    constructor(url: string, opts: unknown) {
      ctor(url, opts);
    }
    on() {}
  },
}));

const ORIGINAL = { ...process.env };

beforeEach(() => {
  ctor.mockClear();
  vi.resetModules();
});
afterEach(() => {
  process.env = { ...ORIGINAL };
});

type Options = {
  tls?: { ca?: string[]; servername?: string };
  keepAlive?: number;
  commandTimeout?: number;
  reconnectOnError?: (err: Error) => boolean;
};

async function construct(url: string, ca?: string) {
  process.env.REDIS_URL = url;
  if (ca === undefined) delete process.env.REDIS_CA_CERT;
  else process.env.REDIS_CA_CERT = ca;
  const { getRedis } = await import("./redis");
  getRedis();
  return ctor.mock.calls[0]?.[1] as Options;
}

async function subscriberOptions() {
  process.env.REDIS_URL = "redis://localhost:6379";
  delete process.env.REDIS_CA_CERT;
  const { createRedisSubscriber } = await import("./redis");
  createRedisSubscriber();
  return ctor.mock.calls[0]?.[1] as Options;
}

describe("redis TLS trust", () => {
  it("trusts the supplied CA on a rediss:// URL", async () => {
    const opts = await construct("rediss://:pw@10.0.0.3:6378", "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----");
    expect(opts.tls?.ca).toEqual([
      "-----BEGIN CERTIFICATE-----\nx\n-----END CERTIFICATE-----",
    ]);
  });

  it("names the server, because Memorystore is reached by IP", async () => {
    const opts = await construct("rediss://:pw@10.0.0.3:6378", "CERT");
    expect(opts.tls?.servername).toBe("10.0.0.3");
  });

  it("passes NO tls key for plaintext local redis", async () => {
    const opts = await construct("redis://localhost:6379");
    expect(opts.tls).toBeUndefined();
  });

  it("passes no tls key for rediss:// with no CA configured", async () => {
    const opts = await construct("rediss://:pw@10.0.0.3:6378");
    expect(opts.tls).toBeUndefined();
  });

  it("gives the subscriber no ready check and no replay of its own", async () => {
    const opts = (await subscriberOptions()) as unknown as {
      enableReadyCheck?: boolean;
      autoResubscribe?: boolean;
      maxRetriesPerRequest?: number | null;
      enableOfflineQueue?: boolean;
    };
    expect(opts.enableReadyCheck).toBe(false);
    expect(opts.autoResubscribe).toBe(false);
    expect(opts.maxRetriesPerRequest).toBeNull();
    expect(opts.enableOfflineQueue).toBe(true);
  });

  it("keeps the fail-fast options the limiter depends on", async () => {
    const opts = (await construct("redis://localhost:6379")) as unknown as {
      maxRetriesPerRequest: number;
      enableOfflineQueue: boolean;
    };
    expect(opts.maxRetriesPerRequest).toBe(1);
    expect(opts.enableOfflineQueue).toBe(false);
  });
});

// Everything below is about a Redis that is REACHABLE and still not working:
// answering slowly, answering as the wrong role, or not answering at all on a
// socket nobody has written to. The options above cover a Redis that is down,
// which is the easy half and the only half ioredis covers by default.
describe("the failures a live connection can still have", () => {
  it("bounds a command the server accepts and never answers", async () => {
    const opts = await construct("redis://localhost:6379");
    expect(
      opts.commandTimeout,
      "a stalled server hangs every request that waits on the limiter",
    ).toBeGreaterThan(0);
  });

  it("lets the subscriber wait forever, because waiting is its job", async () => {
    const opts = await subscriberOptions();
    expect(opts.commandTimeout).toBeUndefined();
  });

  it("probes both connections, because the quiet one is the one that dies unseen", async () => {
    expect((await construct("redis://localhost:6379")).keepAlive).toBeGreaterThan(0);
    expect((await subscriberOptions()).keepAlive).toBeGreaterThan(0);
  });

  it("reconnects when a failover has left it writing to a replica", async () => {
    const { reconnectOnError } = await construct("redis://localhost:6379");
    expect(reconnectOnError).toBeTypeOf("function");
    expect(
      reconnectOnError!(
        new Error("READONLY You can't write against a read only replica."),
      ),
    ).toBe(true);
  });

  it("reconnects on nothing else, or one bad command becomes connection thrash", async () => {
    const { reconnectOnError } = await construct("redis://localhost:6379");
    for (const message of [
      "ERR unknown command 'FOO'",
      "NOSCRIPT No matching script",
      "WRONGTYPE Operation against a key holding the wrong kind of value",
      "LOADING Redis is loading the dataset in memory",
    ]) {
      expect(reconnectOnError!(new Error(message)), message).toBe(false);
    }
  });
});
