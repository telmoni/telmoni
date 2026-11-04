import type { MetadataRoute } from "next";
import { beforeEach, describe, expect, it, vi } from "vitest";

let mockIndexing = true;
vi.mock("@/lib/env", () => ({
  env: {
    get AUTH_URL() {
      return "https://console.example";
    },
    get SEARCH_INDEXING() {
      return mockIndexing;
    },
  },
}));
// Outside a request there is none to wait on.
vi.mock("next/server", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/server")>()),
  connection: async () => {},
}));

import robots from "./robots";

type Group = Exclude<MetadataRoute.Robots["rules"], unknown[]>;

// The crawlers that feed a model, by the token each reads robots.txt under.
const MODEL_CRAWLERS = [
  "GPTBot",
  "OAI-SearchBot",
  "ClaudeBot",
  "PerplexityBot",
  "Google-Extended",
  "Applebot-Extended",
  "CCBot",
];

// The group a crawler obeys: the one naming its token, else the `*` group.
function groupFor(rules: MetadataRoute.Robots["rules"], token: string): Group | undefined {
  const groups = [rules].flat();
  const named = (agent: string) => (group: Group) => [group.userAgent ?? []].flat().includes(agent);
  return groups.find(named(token)) ?? groups.find(named("*"));
}

// Whether `group` keeps its crawler off `path`: a rule is a prefix of the paths it covers.
function keepsOff(group: Group | undefined, path: string): boolean {
  return [group?.disallow ?? []].flat().some((rule) => path.startsWith(rule));
}

describe("robots.txt on the deployment meant to be found", () => {
  beforeEach(() => {
    mockIndexing = true;
  });

  // An invitation's page names who sent it, to which address and for what.
  it("keeps crawlers off invitation pages", async () => {
    expect(keepsOff(groupFor((await robots()).rules, "*"), "/invite/token_abc")).toBe(true);
  });

  it("keeps crawlers off the sign-in lanes and the API", async () => {
    const group = groupFor((await robots()).rules, "*");
    for (const path of ["/auth/login", "/api/health", "/console"]) {
      expect(keepsOff(group, path), path).toBe(true);
    }
  });

  it("leaves the book open, and names its sitemap", async () => {
    const answer = await robots();
    expect(keepsOff(groupFor(answer.rules, "*"), "/docs/start")).toBe(false);
    expect(answer.sitemap).toBe("https://console.example/sitemap.xml");
  });

  it("opens the book to each crawler that feeds a model, under the one `*` group", async () => {
    const { rules } = await robots();
    for (const token of MODEL_CRAWLERS) {
      const group = groupFor(rules, token);
      expect([group?.userAgent].flat(), token).toEqual(["*"]);
      expect(keepsOff(group, "/docs/start"), token).toBe(false);
    }
  });
});

describe("robots.txt on any other deployment", () => {
  beforeEach(() => {
    mockIndexing = false;
  });

  it("keeps every crawler off every path, the splash and the book among them", async () => {
    const { rules } = await robots();
    for (const token of ["Googlebot", "bingbot", ...MODEL_CRAWLERS]) {
      for (const path of ["/", "/docs/start", "/llms.txt"]) {
        expect(keepsOff(groupFor(rules, token), path), `${token} ${path}`).toBe(true);
      }
    }
  });

  it("names no sitemap", async () => {
    expect((await robots()).sitemap).toBeUndefined();
  });
});
