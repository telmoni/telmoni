import { describe, expect, it } from "vitest";
import { z } from "zod";

import { SESSION_EXPIRED } from "@/lib/action-result";
import { refusalWords, routeReadOf } from "@/lib/route-read";

const Answer = z.object({ n: z.number() });

describe("routeReadOf", () => {
  it("reads an answer that parses, and refuses one that does not", async () => {
    expect(await routeReadOf(Response.json({ n: 1 }), Answer)).toEqual({ kind: "ok", value: { n: 1 } });
    expect(await routeReadOf(Response.json({ n: "one" }), Answer)).toEqual({
      kind: "unavailable",
      status: 200,
      error: null,
    });
    expect(await routeReadOf(new Response("not json", { status: 200 }), Answer)).toMatchObject({
      kind: "unavailable",
    });
  });

  /// The session's end comes as the proxy's 401, or as a redirect to sign in the read never followed.
  it("tells a lapsed session from a refusal", async () => {
    expect(await routeReadOf(new Response(null, { status: 401 }), Answer)).toEqual({ kind: "signed-out" });
    const redirected = { type: "opaqueredirect", status: 0, json: async () => null } as unknown as Response;
    expect(await routeReadOf(redirected, Answer)).toEqual({ kind: "signed-out" });
  });

  it("carries a refusal's words where the handler gave any", async () => {
    const problem = { type: "about:blank", title: "Forbidden", status: 403, detail: "Not yours to read." };
    expect(await routeReadOf(Response.json(problem, { status: 403 }), Answer)).toEqual({
      kind: "forbidden",
      error: "Not yours to read.",
    });
    expect(await routeReadOf(new Response("{}", { status: 404 }), Answer)).toEqual({ kind: "missing", error: null });
    expect(await routeReadOf(Response.json({ ...problem, status: 503 }, { status: 503 }), Answer)).toEqual({
      kind: "unavailable",
      status: 503,
      error: "Not yours to read.",
    });
  });
});

describe("refusalWords", () => {
  it("says the session ended, else the handler's words, else the caller's", () => {
    expect(refusalWords({ kind: "signed-out" }, "fallback")).toBe(SESSION_EXPIRED);
    expect(refusalWords({ kind: "forbidden", error: "theirs" }, "fallback")).toBe("theirs");
    expect(refusalWords({ kind: "unavailable", status: null, error: null }, "fallback")).toBe("fallback");
  });
});
