// The receiver's half of the signed-webhook contract, as a receiver would
// write it. `/docs/webhooks` prints this function as the reference verifier,
// and `webhook-signature.test.ts` proves it against the golden vector the
// Rust signer pins — so the snippet a customer copies is one that has been run
// against the bytes the platform actually sends, not prose that resembles it.
//
// Node-only (`node:crypto`), server-side. Nothing here is a secret: the golden
// secret below is a published test fixture, never a real one.
import { createHmac, timingSafeEqual } from "node:crypto";

/// Seconds a receiver should let `t=` stray from its own clock before refusing
/// the delivery as a replay. Five minutes — Stripe's number — and the same
/// constant `crates/notifications/src/connector/webhook.rs` documents as the
/// platform's half of the bargain: every attempt is signed when it is sent, so
/// a retry after a long backoff still lands inside this window.
export const REPLAY_TOLERANCE_SECS = 300;

/// The published golden vector. The expected signature was computed
/// independently in Python before the Rust signer or this verifier existed;
/// Rust pins it in `the_published_golden_vector_verifies`, and the test beside
/// this file pins it here. Three implementations, one answer.
export const GOLDEN = {
  secret: "whsec_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  timestamp: 1_800_000_000,
  body: '{"id":"019a0b8c-3f2e-7d41-9c5a-6e2b1f0d8a47","kind":"member_added","title":"Sam joined","body":"Sam accepted your invitation.","sent_at":"2027-01-15T12:00:00Z"}',
  header: "t=1800000000,v1=396556d0e74d35a762e7d75d387e1a7dcb4c728ffcc9bc857830a03f13dd740f",
} as const;

/**
 * Verify a `Telmoni-Signature` header over the raw request body.
 *
 * `rawBody` must be the exact bytes received — not a re-serialised parse —
 * because the signature is over the bytes sent. `nowSecs` is injectable so a
 * test can stand at a chosen moment; a receiver leaves it to default.
 *
 * Accepts the delivery when ANY `v1` in the header matches. While a rotated
 * secret's overlap lasts the header carries two, the new secret's and the
 * prior one's, so a receiver holding either keeps verifying.
 *
 * Returns `false` for a malformed header, no signature that matches, or a
 * timestamp outside `REPLAY_TOLERANCE_SECS` of now. It never throws on
 * input, because a webhook endpoint that 500s on a bad header is one an
 * attacker can make noisy.
 */
export function verifyTelmoniSignature(
  secret: string,
  header: string,
  rawBody: string | Uint8Array,
  nowSecs: number = Math.floor(Date.now() / 1000),
): boolean {
  let t: number | undefined;
  const candidates: string[] = [];
  for (const part of header.split(",")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    const key = part.slice(0, eq).trim();
    const value = part.slice(eq + 1).trim();
    if (key === "t") t = Number(value);
    else if (key === "v1") candidates.push(value);
  }
  if (t === undefined || !Number.isInteger(t) || candidates.length === 0) return false;
  if (Math.abs(nowSecs - t) > REPLAY_TOLERANCE_SECS) return false;

  const expected = createHmac("sha256", secret)
    .update(`${t}.`)
    .update(rawBody)
    .digest();
  return candidates.some((v1) => {
    const given = Buffer.from(v1, "hex");
    // A hex string of the wrong length is a mismatch, and `timingSafeEqual`
    // throws on unequal lengths rather than answering, so say so first.
    return given.length === expected.length && timingSafeEqual(given, expected);
  });
}
