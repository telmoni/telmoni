import { afterEach, describe, expect, it } from "vitest";

import { legalRedirect } from "@/proxy";

describe("legalRedirect", () => {
  afterEach(() => {
    delete process.env.LEGAL_URL;
  });

  it("sends each document to the deployment's published copy", () => {
    process.env.LEGAL_URL = "https://docs.example.com/legal/";
    expect(legalRedirect("/legal/privacy-policy")).toBe(
      "https://docs.example.com/legal/privacy-policy/",
    );
    expect(legalRedirect("/legal/terms-of-service/")).toBe(
      "https://docs.example.com/legal/terms-of-service/",
    );
    expect(legalRedirect("/legal")).toBe("https://docs.example.com/legal/");
  });

  // The two names the console once served under other documents keep
  // landing on those documents, so an old link does not answer 404.
  it("folds the retired names into the documents that carried them", () => {
    process.env.LEGAL_URL = "https://docs.example.com/legal";
    expect(legalRedirect("/legal/dpa")).toBe(
      "https://docs.example.com/legal/privacy-policy/",
    );
    expect(legalRedirect("/legal/acceptable-use-policy")).toBe(
      "https://docs.example.com/legal/terms-of-service/",
    );
  });

  // A deployment that published nothing gets no redirect and no page: the
  // path answers 404 rather than another operator's policy.
  it("redirects nowhere when no legal base is configured", () => {
    expect(legalRedirect("/legal/privacy-policy")).toBeNull();
    process.env.LEGAL_URL = "   ";
    expect(legalRedirect("/legal/privacy-policy")).toBeNull();
  });
});
