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
vi.mock("@/lib/source", () => ({
  source: { getPages: () => [{ url: "/docs/monitoring/alerts" }] },
}));

import sitemap from "./sitemap";

describe("sitemap.xml", () => {
  beforeEach(() => {
    mockIndexing = true;
  });

  it("lists the splash and each page of the book on the deployment meant to be found", async () => {
    expect((await sitemap()).map((entry) => entry.url)).toEqual([
      "https://console.example",
      "https://console.example/docs/monitoring/alerts",
    ]);
  });

  it("lists nothing on any other", async () => {
    mockIndexing = false;
    expect(await sitemap()).toEqual([]);
  });
});
