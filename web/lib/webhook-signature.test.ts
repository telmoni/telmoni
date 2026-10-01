import { createHmac } from "node:crypto";

import { describe, expect, it } from "vitest";

import {
  GOLDEN,
  REPLAY_TOLERANCE_SECS,
  verifyTelmoniSignature,
} from "./webhook-signature";

// The moment the golden vector was signed: inside the window by construction.
const AT_SIGNING = GOLDEN.timestamp;

describe("the published golden vector", () => {
  // ⚠ The hex here is the one Python produced and the one Rust asserts in
  // `the_published_golden_vector_verifies`. If this fails and Rust passes, the
  // verifier on the docs page is wrong; if both fail, the scheme changed and
  // every receiver written against the page is broken.
  it("is what HMAC-SHA256 over `{t}.{body}` gives, and the verifier accepts it", () => {
    const hex = createHmac("sha256", GOLDEN.secret)
      .update(`${GOLDEN.timestamp}.${GOLDEN.body}`)
      .digest("hex");
    expect(`t=${GOLDEN.timestamp},v1=${hex}`).toBe(GOLDEN.header);
    expect(verifyTelmoniSignature(GOLDEN.secret, GOLDEN.header, GOLDEN.body, AT_SIGNING)).toBe(
      true,
    );
  });

  it("accepts the body as bytes, which is how a receiver actually has it", () => {
    const bytes = new TextEncoder().encode(GOLDEN.body);
    expect(verifyTelmoniSignature(GOLDEN.secret, GOLDEN.header, bytes, AT_SIGNING)).toBe(true);
  });
});

describe("what the verifier refuses", () => {
  it("a tampered body", () => {
    const tampered = GOLDEN.body.replace("Sam joined", "Sam left");
    expect(verifyTelmoniSignature(GOLDEN.secret, GOLDEN.header, tampered, AT_SIGNING)).toBe(false);
  });

  it("the wrong secret", () => {
    expect(verifyTelmoniSignature("whsec_other", GOLDEN.header, GOLDEN.body, AT_SIGNING)).toBe(
      false,
    );
  });

  it("a header whose `t=` was moved — the timestamp is inside the MAC", () => {
    const moved = GOLDEN.header.replace("t=1800000000", "t=1800000001");
    expect(verifyTelmoniSignature(GOLDEN.secret, moved, GOLDEN.body, AT_SIGNING + 1)).toBe(false);
  });

  it.each(["", "v1=abc", "t=1800000000", "t=x,v1=00", "t=1800000000,v1=zz"])(
    "a malformed header %j, without throwing",
    (header) => {
      expect(verifyTelmoniSignature(GOLDEN.secret, header, GOLDEN.body, AT_SIGNING)).toBe(false);
    },
  );

  it("a signature of the wrong length rather than throwing on it", () => {
    const short = "t=1800000000,v1=396556d0";
    expect(verifyTelmoniSignature(GOLDEN.secret, short, GOLDEN.body, AT_SIGNING)).toBe(false);
  });
});

describe("a rotation's overlap", () => {
  // During an overlap the platform sends one `t=` and two `v1`s: the new
  // secret's first, then the prior one's. A receiver holding either verifies.
  const PRIOR = "whsec_prior_secret_still_signing";
  const sig = (secret: string) =>
    createHmac("sha256", secret).update(`${GOLDEN.timestamp}.${GOLDEN.body}`).digest("hex");
  const overlap = `t=${GOLDEN.timestamp},v1=${sig(GOLDEN.secret)},v1=${sig(PRIOR)}`;

  it("verifies under the new secret and under the prior one", () => {
    expect(verifyTelmoniSignature(GOLDEN.secret, overlap, GOLDEN.body, AT_SIGNING)).toBe(true);
    expect(verifyTelmoniSignature(PRIOR, overlap, GOLDEN.body, AT_SIGNING)).toBe(true);
  });

  it("still refuses a secret that is neither, and a tampered body", () => {
    expect(verifyTelmoniSignature("whsec_other", overlap, GOLDEN.body, AT_SIGNING)).toBe(false);
    const tampered = GOLDEN.body.replace("Sam joined", "Sam left");
    expect(verifyTelmoniSignature(PRIOR, overlap, tampered, AT_SIGNING)).toBe(false);
  });

  it("does not depend on the order of the signatures", () => {
    const reversed = `t=${GOLDEN.timestamp},v1=${sig(PRIOR)},v1=${sig(GOLDEN.secret)}`;
    expect(verifyTelmoniSignature(GOLDEN.secret, reversed, GOLDEN.body, AT_SIGNING)).toBe(true);
  });
});

describe("the replay window", () => {
  it("is five minutes, the number the docs publish", () => {
    expect(REPLAY_TOLERANCE_SECS).toBe(300);
  });

  it("accepts a delivery exactly at the edge and refuses one past it, in both directions", () => {
    const edge = REPLAY_TOLERANCE_SECS;
    const ok = (now: number) =>
      verifyTelmoniSignature(GOLDEN.secret, GOLDEN.header, GOLDEN.body, now);
    expect(ok(AT_SIGNING + edge)).toBe(true);
    expect(ok(AT_SIGNING - edge)).toBe(true);
    expect(ok(AT_SIGNING + edge + 1)).toBe(false);
    expect(ok(AT_SIGNING - edge - 1)).toBe(false);
  });
});
