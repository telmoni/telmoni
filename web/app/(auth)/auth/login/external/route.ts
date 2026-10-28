import { type NextRequest, NextResponse } from "next/server";
import { getAuthorizeUrl } from "@/lib/auth/oidc";
import { newProof } from "@/lib/auth/pkce";
import { PKCE_COOKIE, sealPkce, unsealPkce } from "@/lib/auth/session";
import { pastStartCeiling, signInUnavailable, toProvider } from "@/lib/auth/start";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

// The sign-in page's "Continue with…". The door already sealed the state
// and where to land, so this carries the same sign-in on to the external
// provider, under a fresh verifier and nonce, and marks it as the
// provider's so the callback spends the code there. A visit with no sign-in
// in progress goes back through the door.
export async function GET(request: NextRequest) {
  const sealed = request.cookies.get(PKCE_COOKIE)?.value;
  const pkce = sealed ? await unsealPkce(sealed) : null;
  if (!pkce) {
    return NextResponse.redirect(new URL("/auth/login", env.AUTH_URL));
  }

  // The doors' own bucket: this is the same call to auth.
  const refused = await pastStartCeiling(request);
  if (refused) return refused;

  const proof = await newProof();
  let authUrl: string;
  try {
    authUrl = await getAuthorizeUrl(pkce.state, {
      codeChallenge: proof.challenge,
      nonce: proof.nonce,
      provider: "external",
    });
  } catch (err) {
    logger.error({ err }, "auth start failed");
    return signInUnavailable();
  }

  const resealed = await sealPkce({
    ...pkce,
    verifier: proof.verifier,
    nonce: proof.nonce,
    provider: "external",
  });
  return toProvider(authUrl, resealed);
}
