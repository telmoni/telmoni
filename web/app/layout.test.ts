import { describe, expect, it, vi } from "vitest";

let mockIndexing = false;
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
// Next's font loader runs at build, where a test never is.
vi.mock("next/font/google", () => ({
  Geist_Mono: () => ({ variable: "" }),
}));

import { generateMetadata } from "./layout";

describe("the root layout's metadata", () => {
  it("offers every page to the engines on the deployment meant to be found", () => {
    mockIndexing = true;
    expect(generateMetadata().robots).toBe("index, follow");
  });

  it("keeps every page out of their indexes on any other", () => {
    mockIndexing = false;
    expect(generateMetadata().robots).toBe("noindex, nofollow");
  });
});
