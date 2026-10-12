import { beforeAll, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/env", () => ({ env: { AUTH_URL: "https://console.example" } }));
// Outside a request there is none to wait on.
vi.mock("next/server", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/server")>()),
  connection: async () => {},
}));

import robots from "./robots";

let disallowed: string[] = [];
beforeAll(async () => {
  disallowed = [(await robots()).rules].flat().flatMap((rule) => [rule.disallow ?? []].flat());
});

// A rule keeps crawlers off every path it is a prefix of, and nothing else.
describe("robots.txt", () => {
  // An invitation's page names who sent it, to which address and for what.
  it("keeps crawlers off invitation pages", () => {
    expect(disallowed.some((rule) => "/invite/token_abc".startsWith(rule))).toBe(true);
  });

  it("keeps crawlers off the sign-in lanes and the API", () => {
    for (const path of ["/auth/login", "/api/health", "/console"]) {
      expect(disallowed.some((rule) => path.startsWith(rule)), path).toBe(true);
    }
  });

  it("leaves the book open", () => {
    expect(disallowed.some((rule) => "/docs/start".startsWith(rule))).toBe(false);
  });
});
