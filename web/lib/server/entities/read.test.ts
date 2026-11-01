import { beforeEach, describe, expect, it, vi } from "vitest";
import { z } from "zod";

const { fetchWithTimeout, warn } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  warn: vi.fn(),
}));

vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("@/lib/logger", () => ({ logger: { warn } }));
import { readBytes, readJson, valueOf } from "./read";

const Schema = z.object({ n: z.number() });

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

const read = () =>
  readJson("fetchThing", "http://server/thing", { cache: "no-store" }, Schema, { 404: "missing" });

beforeEach(() => {
  vi.clearAllMocks();
});

describe("readJson", () => {
  it("answers the value its schema reads, and asks as it was told", async () => {
    answer(200, { n: 1 });
    expect(await read()).toEqual({ kind: "ok", value: { n: 1 } });
    expect(fetchWithTimeout).toHaveBeenCalledWith("http://server/thing", { cache: "no-store" });
    expect(warn).not.toHaveBeenCalled();
  });

  it("answers a status the caller named as its kind, and logs nothing", async () => {
    answer(404, null);
    expect(await read()).toEqual({ kind: "missing" });
    expect(warn).not.toHaveBeenCalled();
  });

  it("answers a 403 as forbidden", async () => {
    answer(403, null);
    expect(await read()).toEqual({ kind: "forbidden" });
  });

  // An outage leaves a line naming the fetcher and the status.
  it("logs any other refusal by its status", async () => {
    answer(503, null);
    expect(await read()).toEqual({ kind: "unavailable" });
    expect(warn).toHaveBeenCalledWith(
      { fetcher: "fetchThing", status: 503 },
      "entities: upstream refused",
    );
  });

  it("logs an answer of the wrong shape, never an empty value", async () => {
    answer(200, { n: "one" });
    expect(await read()).toEqual({ kind: "unavailable" });
    expect(warn).toHaveBeenCalledWith(
      expect.objectContaining({ fetcher: "fetchThing", issues: expect.any(Array) }),
      "entities: upstream shape mismatch",
    );
  });

  it("logs a read that threw by its message", async () => {
    fetchWithTimeout.mockRejectedValue(new Error("connect ECONNREFUSED"));
    expect(await read()).toEqual({ kind: "unavailable" });
    expect(warn).toHaveBeenCalledWith(
      { fetcher: "fetchThing", error: "connect ECONNREFUSED" },
      "entities: fetch failed",
    );
  });
});

describe("readBytes", () => {
  function bytes(status: number, type: string | null) {
    fetchWithTimeout.mockResolvedValue({
      ok: status < 400,
      status,
      headers: new Headers(type ? { "content-type": type } : {}),
      arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer,
    });
  }
  const readFile = () =>
    readBytes("fetchFile", "http://server/file", { cache: "no-store" }, { 404: "missing" }, 30_000);

  it("answers the bytes and their type as they came, waiting as long as it was told", async () => {
    bytes(200, "image/png");
    const read = await readFile();
    expect(read).toEqual({ kind: "ok", body: expect.any(ArrayBuffer), contentType: "image/png" });
    expect(fetchWithTimeout).toHaveBeenCalledWith("http://server/file", { cache: "no-store" }, 30_000);
  });

  it("names bytes of no type as a stream of octets", async () => {
    bytes(200, null);
    expect(await readFile()).toMatchObject({ kind: "ok", contentType: "application/octet-stream" });
  });

  it("refuses as readJson does: a named status, a 403, then any other logged", async () => {
    bytes(404, null);
    expect(await readFile()).toEqual({ kind: "missing" });
    bytes(403, null);
    expect(await readFile()).toEqual({ kind: "forbidden" });
    bytes(401, null);
    expect(await readFile()).toEqual({ kind: "unavailable" });
    expect(warn).toHaveBeenCalledWith(
      { fetcher: "fetchFile", status: 401 },
      "entities: upstream refused",
    );
  });
});

describe("valueOf", () => {
  it("is the value of a read that answered, else null", () => {
    expect(valueOf({ kind: "ok", value: 1 })).toBe(1);
    expect(valueOf({ kind: "forbidden" })).toBeNull();
    expect(valueOf({ kind: "unavailable" })).toBeNull();
  });
});
