import { describe, expect, it } from "vitest";

import { readBodyCapped } from "./body";

const CAP = 16;

/** A body delivered in the chunks given, with no content-length. */
function chunked(...parts: string[]): Request {
  const encoder = new TextEncoder();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const part of parts) controller.enqueue(encoder.encode(part));
      controller.close();
    },
  });
  return new Request("http://localhost/hook", {
    method: "POST",
    body: stream,
    // Node's fetch requires this to send a stream; it is what a chunked
    // request looks like from the handler's side.
    duplex: "half",
  } as RequestInit);
}

describe("readBodyCapped", () => {
  it("returns the bytes of a body under the ceiling", async () => {
    const body = await readBodyCapped(chunked("hello", " ", "world"), CAP);
    expect(body && new TextDecoder().decode(body)).toBe("hello world");
  });

  it("returns an empty body for a request with none", async () => {
    const body = await readBodyCapped(new Request("http://localhost/hook"), CAP);
    expect(body).toEqual(new Uint8Array(0));
  });

  it("refuses a declared length over the ceiling without reading", async () => {
    const request = new Request("http://localhost/hook", {
      method: "POST",
      headers: { "content-length": String(CAP + 1) },
      body: "x",
    });
    expect(await readBodyCapped(request, CAP)).toBeNull();
    // Untouched: a refused body is still there to be read.
    expect(request.bodyUsed).toBe(false);
  });

  // The case the header check cannot see: nothing declared, and the body
  // keeps coming.
  it("refuses a chunked body that crosses the ceiling", async () => {
    const body = await readBodyCapped(chunked("0123456789", "0123456789"), CAP);
    expect(body).toBeNull();
  });

  it("takes a body that lands exactly on the ceiling", async () => {
    const body = await readBodyCapped(chunked("0123456789", "012345"), CAP);
    expect(body?.byteLength).toBe(CAP);
  });

  it("refuses a body whose declared length understates it", async () => {
    const request = new Request("http://localhost/hook", {
      method: "POST",
      headers: { "content-length": "4" },
      body: "0123456789012345678901234567890123456789",
    });
    expect(await readBodyCapped(request, CAP)).toBeNull();
  });
});
