import { describe, expect, it } from "vitest";

import { challengeOf, newProof } from "./pkce";

describe("a sign-in's proof", () => {
  // RFC 7636, Appendix B: the specification's own example.
  it("derives the S256 challenge the RFC does", async () => {
    expect(await challengeOf("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")).toBe(
      "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
    );
  });

  it("is fresh each time, 43 base64url characters a token, the challenge the verifier's", async () => {
    const [a, b] = [await newProof(), await newProof()];
    expect(a.verifier).not.toBe(b.verifier);
    expect(a.nonce).not.toBe(b.nonce);
    for (const token of [a.verifier, a.nonce, a.challenge]) {
      expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    }
    expect(a.challenge).toBe(await challengeOf(a.verifier));
  });
});
