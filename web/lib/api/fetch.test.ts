import { describe, it, expect, vi, afterEach } from "vitest";
import {
  safeReturnTo,
  fetchWithTimeout,
  extractProblem,
} from "./fetch";

describe("safeReturnTo", () => {
  const base = new URL("http://localhost:3000");

  it("returns / for undefined input", () => {
    expect(safeReturnTo(undefined, base)).toBe("/");
  });

  it("returns / for empty string", () => {
    expect(safeReturnTo("", base)).toBe("/");
  });

  it("returns same-origin path unchanged", () => {
    expect(safeReturnTo("/runs", base)).toBe("/runs");
  });

  it("preserves query string on same-origin path", () => {
    expect(safeReturnTo("/search?q=hello", base)).toBe("/search?q=hello");
  });

  it("returns / for cross-origin absolute URL", () => {
    expect(safeReturnTo("https://evil.com/steal", base)).toBe("/");
  });

  it("returns / for protocol-relative URL", () => {
    expect(safeReturnTo("//evil.com/steal", base)).toBe("/");
  });

  it("returns / for malformed absolute URL", () => {
    expect(safeReturnTo("http://[invalid]/path", base)).toBe("/");
  });

  // ⚠ These assert on what the CALLER lands on, not on the returned string,
  // because that is where the bug was: `/..//evil.com` is same-origin when
  // parsed against `base`, so the origin check passed, and only the normalised
  // `url.pathname` — `//evil.com` — was protocol-relative. Asserting the
  // re-resolution is the only shape of this test that would have failed.
  const lands = (raw: string) => new URL(safeReturnTo(raw, base), base).origin;

  it.each([
    "/..//evil.com",
    "/./..//evil.com/x",
    "/a/../..//evil.com",
    "/path/..//evil.com?next=1",
  ])("keeps %s on our own origin after the caller re-resolves it", (raw) => {
    expect(lands(raw)).toBe(base.origin);
  });

  it("still returns a legitimate path containing dot segments", () => {
    expect(safeReturnTo("/project_x/../project_y/api-keys", base)).toBe("/project_y/api-keys");
  });
});

describe("fetchWithTimeout", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** A fetch that only ever ends by being aborted, with the signal's reason. */
  function neverAnswers() {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation(
        (_url: string, opts: RequestInit) =>
          new Promise((_resolve, reject) => {
            opts.signal?.addEventListener("abort", () => reject(opts.signal?.reason));
          }),
      ),
    );
  }

  it("returns response when fetch completes within timeout", async () => {
    const mockResponse = new Response("ok", { status: 200 });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(mockResponse));

    const result = await fetchWithTimeout("http://example.com", {}, 5000);
    expect(result.status).toBe(200);
  });

  // Real timers on purpose: `AbortSignal.timeout` keeps its own clock and a
  // faked `setTimeout` would never fire it.
  it("aborts with a TimeoutError when the headers never arrive", async () => {
    neverAnswers();
    await expect(fetchWithTimeout("http://example.com", {}, 20)).rejects.toMatchObject({
      name: "TimeoutError",
    });
  });

  // ⚠ The contract the readers depend on: the signal handed to fetch is still
  // armed once the headers are in, so a body that stalls is cut at `ms` too.
  it("keeps the signal armed after the headers arrive", async () => {
    let seen: AbortSignal | null = null;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((_url: string, opts: RequestInit) => {
        seen = opts.signal ?? null;
        return Promise.resolve(new Response("ok"));
      }),
    );
    await fetchWithTimeout("http://example.com", {}, 20);
    expect(seen!.aborted).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(seen!.aborted).toBe(true);
  });

  it("honours the caller's own signal, with the caller's reason", async () => {
    neverAnswers();
    const controller = new AbortController();
    const pending = fetchWithTimeout("http://example.com", { signal: controller.signal }, 5000);
    controller.abort(new Error("client went away"));
    await expect(pending).rejects.toThrow("client went away");
  });

  it("never lets a service answer into the build cache unless asked", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response("ok"));
    vi.stubGlobal("fetch", fetchMock);
    await fetchWithTimeout("http://example.com");
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ cache: "no-store" });
    await fetchWithTimeout("http://example.com", { cache: "force-cache" });
    expect(fetchMock.mock.calls[1]?.[1]).toMatchObject({ cache: "force-cache" });
  });
});

describe("extractProblem", () => {
  const json = (body: unknown, status = 400) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/problem+json" },
    });

  it("parses a problem body and keeps its extension members", async () => {
    const { message, problem } = await extractProblem(
      json({
        type: "/errors/tenant/feature-off",
        title: "feature switched off",
        status: 503,
        detail: "API keys are switched off right now.",
        flag: "api_tokens",
      }, 503),
    );
    expect(message).toBe("feature switched off: API keys are switched off right now.");
    expect(problem?.type).toBe("/errors/tenant/feature-off");
    expect(problem?.flag).toBe("api_tokens");
  });

  it("accepts a problem with no `type`, and types it as possibly absent", async () => {
    const { problem } = await extractProblem(
      json({ title: "Bad request", status: 400 }),
    );
    expect(problem?.title).toBe("Bad request");
    expect(problem?.type).toBeUndefined();
  });

  it("refuses a body whose required fields are the wrong type", async () => {
    const { message, problem } = await extractProblem(
      json({ title: "Nope", status: "400" }, 400),
    );
    expect(problem).toBeNull();
    expect(message).toBe("request failed (400)");
  });

  it("still reads the legacy {error} shape", async () => {
    const { message, problem } = await extractProblem(
      json({ error: "slug already taken" }, 409),
    );
    expect(message).toBe("slug already taken");
    expect(problem).toBeNull();
  });

  it("falls back on a non-JSON body without throwing", async () => {
    const res = new Response("gateway blew up", {
      status: 502,
      headers: { "content-type": "text/plain" },
    });
    const { message, problem } = await extractProblem(res);
    expect(message).toBe("gateway blew up");
    expect(problem).toBeNull();
  });
});
