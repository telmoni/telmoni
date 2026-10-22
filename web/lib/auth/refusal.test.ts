import { describe, expect, it } from "vitest";

import { ExchangeRefused, refusalNamed, refusalOf, refusedUrl } from "./refusal";

describe("refusalOf", () => {
  it("tells a closed sign-up and a held address apart from any other failure", () => {
    expect(refusalOf(new ExchangeRefused(403, "/errors/auth/sign-up-closed"))).toBe("closed");
    expect(refusalOf(new ExchangeRefused(409, "/errors/auth/conflict"))).toBe("taken");
    expect(refusalOf(new ExchangeRefused(403, "/errors/authz/forbidden"))).toBe("failed");
    expect(refusalOf(new ExchangeRefused(502, null))).toBe("failed");
    expect(refusalOf(new Error("network"))).toBe("failed");
  });
});

describe("refusalNamed", () => {
  // The page shows its own words for a reason it knows, and nothing a link
  // could slip in.
  it("reads a known reason and nothing else", () => {
    expect(refusalNamed("closed")).toBe("closed");
    expect(refusalNamed("toString")).toBe("failed");
    expect(refusalNamed("<b>hi</b>")).toBe("failed");
    expect(refusalNamed(undefined)).toBe("failed");
  });
});

describe("refusedUrl", () => {
  it("names the reason on the console's own page", () => {
    expect(refusedUrl("closed", "https://app.example").toString()).toBe(
      "https://app.example/auth/refused?reason=closed",
    );
  });
});
