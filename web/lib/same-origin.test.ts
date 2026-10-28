import { describe, expect, it } from "vitest";

import { sameOriginUrl } from "./same-origin";

describe("sameOriginUrl", () => {
  it("resolves a path on this console, `..` and `.` included", () => {
    expect(sameOriginUrl("/acme/web?page=2#top")?.pathname).toBe("/acme/web");
    expect(sameOriginUrl("/acme/../web/members")?.pathname).toBe("/web/members");
    expect(sameOriginUrl("/acme/./audit-log")?.pathname).toBe("/acme/audit-log");
  });

  it("refuses anything a browser would read as another origin", () => {
    for (const raw of [
      "https://evil.example/",
      "//evil.example/",
      "/\\evil.example",
      "/..//evil.example",
      "/acme/%2e%2e//evil.example/",
      "/a\u0000b",
      "/a\nb",
      "acme/web",
      "",
    ]) {
      expect(sameOriginUrl(raw), raw).toBeNull();
    }
  });

  it("holds a path to the base it is given", () => {
    const base = "http://localhost:3000/";
    expect(sameOriginUrl("/runs", base)?.href).toBe("http://localhost:3000/runs");
    expect(sameOriginUrl("/..//evil.example", base)).toBeNull();
  });
});
