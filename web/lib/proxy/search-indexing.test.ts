import { NextRequest, NextResponse } from "next/server";
import { afterEach, describe, expect, it, vi } from "vitest";

import { proxy } from "@/proxy";

async function run(path: string): Promise<NextResponse> {
  const res = await proxy(new NextRequest(`http://localhost:3000${path}`), {} as never);
  if (!(res instanceof NextResponse)) throw new Error("the proxy answered no NextResponse");
  return res;
}

// Each answer a signed-out caller, a crawler among them, can be given: a
// public page and file, the sign-in redirect, a 401 and the legal redirect.
const ANSWERS = [
  "/",
  "/docs/start",
  "/robots.txt",
  "/llms.txt",
  "/acme/settings",
  "/api/v1/events",
  "/legal/privacy-policy",
];

describe("the proxy's word to the engines", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("says noindex with every answer of a deployment not meant to be found", async () => {
    vi.stubEnv("SEARCH_INDEXING", "");
    vi.stubEnv("LEGAL_URL", "https://legal.example");
    for (const path of ANSWERS) {
      expect((await run(path)).headers.get("x-robots-tag"), path).toBe("noindex");
    }
  });

  it("says nothing of it on the deployment meant to be found", async () => {
    vi.stubEnv("SEARCH_INDEXING", "true");
    vi.stubEnv("LEGAL_URL", "https://legal.example");
    for (const path of ANSWERS) {
      expect((await run(path)).headers.get("x-robots-tag"), path).toBeNull();
    }
  });
});
