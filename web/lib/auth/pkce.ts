import "server-only";

// A sign-in's proof of possession (RFC 7636) and its replay guard (OpenID
// Connect's nonce), as RFC 9700 asks of every client: `state` alone stops a
// forged callback, never a stolen code replayed in its thief's own sign-in.
// The verifier and the nonce are sealed into the sign-in cookie; the
// challenge, the verifier's SHA-256, is all that leaves with the browser.
export interface Proof {
  verifier: string;
  nonce: string;
  challenge: string;
}

// 32 random bytes, base64url: RFC 7636's own recommendation for a verifier.
function randomToken(): string {
  return Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url");
}

// The S256 challenge of a verifier: base64url of its SHA-256.
export async function challengeOf(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return Buffer.from(digest).toString("base64url");
}

// A fresh verifier and nonce for one sign-in, and the challenge sent ahead.
export async function newProof(): Promise<Proof> {
  const verifier = randomToken();
  return { verifier, nonce: randomToken(), challenge: await challengeOf(verifier) };
}
