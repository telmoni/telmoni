import { beforeEach, describe, expect, it, vi } from "vitest";

const fetchWithTimeout = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api/fetch", () => ({
  fetchWithTimeout: (...a: unknown[]) => fetchWithTimeout(...a),
}));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://auth.test", SERVICE_SECRET: "s3cret" },
}));

import {
  exchangeCode,
  getAuthorizeUrl,
  getLogoutUrl,
  getSignInConfig,
  refreshTokens,
  revokeSession,
} from "./oidc";
import { ExchangeRefused } from "./refusal";

const ok = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });

beforeEach(() => fetchWithTimeout.mockReset());

const PROOF = { codeChallenge: "challenge_1", nonce: "nonce_1" };
const SPENT = { codeVerifier: "verifier_1", nonce: "nonce_1" };

// ⚠ Both values are handed straight to `NextResponse.redirect`, which throws
// on anything it cannot parse as a URL. The schemas typed them `z.string()`,
// so a malformed upstream answer travelled all the way to the redirect —
// past `/auth/login`'s try/catch, which only wraps the fetch.
describe("the two URLs that become redirects", () => {
  it("returns the authorize URL when it is one", async () => {
    fetchWithTimeout.mockResolvedValue(
      ok({ authorizeUrl: "https://idp.example/authorize?x=1" }),
    );
    await expect(getAuthorizeUrl("state_1", PROOF)).resolves.toBe(
      "https://idp.example/authorize?x=1",
    );
  });

  // Parseability only, deliberately: a `javascript:` URL passes `z.url()` and
  // is fine, because every browser ignores a `Location` that is not http(s).
  // What must not pass is anything `new URL()` refuses, which is the only
  // thing that makes `NextResponse.redirect` throw.
  it.each(["", "not-a-url", "/relative/path", "http://[invalid]/x"])(
    "refuses %p as an authorize URL, so the route answers its 503",
    async (authorizeUrl) => {
      fetchWithTimeout.mockResolvedValue(ok({ authorizeUrl }));
      await expect(getAuthorizeUrl("state_1", PROOF)).rejects.toThrow(
        "auth start: malformed response",
      );
    },
  );

  it("returns the logout URL when it is one", async () => {
    fetchWithTimeout.mockResolvedValue(
      ok({ logoutUrl: "https://idp.example/logout?session=1" }),
    );
    await expect(getLogoutUrl("sid_1", "id.token.x", "https://app.test/")).resolves.toBe(
      "https://idp.example/logout?session=1",
    );
    const [, init] = fetchWithTimeout.mock.calls[0]!;
    expect(JSON.parse(String(init?.body))).toEqual({
      session_id: "sid_1",
      id_token: "id.token.x",
      return_to: "https://app.test/",
    });
  });

  it.each(["", "not-a-url", "/logged-out"])(
    "answers null for %p, so sign-out falls back to / instead of raising",
    async (logoutUrl) => {
      fetchWithTimeout.mockResolvedValue(ok({ logoutUrl }));
      await expect(getLogoutUrl("sid_1", "id.token.x", "https://app.test/")).resolves.toBeNull();
    },
  );
});

describe("refreshTokens", () => {
  const rotated = () =>
    ok({
      userId: "user_1",
      email: "ada@example.test",
      emailVerified: true,
      firstName: "Ada",
      lastName: null,
      accessToken: "at_new",
      sessionId: "ses_1",
      refreshToken: "rt_new",
      expiresIn: 3600,
    });

  function sent() {
    const [url, init] = fetchWithTimeout.mock.calls[0] as [
      string,
      { body: string; headers: Record<string, string> },
    ];
    return { url, body: JSON.parse(init.body), headers: init.headers };
  }

  // The row travels with the grant so auth marks it seen as it rotates the
  // tokens; the console makes no call of its own for that any more.
  it("posts the grant and the session row, and answers the rotated tokens", async () => {
    fetchWithTimeout.mockResolvedValue(rotated());
    const answer = await refreshTokens("rt_old", "sess_1");
    expect(answer).toMatchObject({
      kind: "granted",
      tokens: { accessToken: "at_new", sessionId: "ses_1" },
    });
    expect(sent().url).toBe("http://auth.test/internal/v1/auth/refresh");
    expect(sent().body).toEqual({ refresh_token: "rt_old", session_row_id: "sess_1" });
    expect(sent().headers["x-service-secret"]).toBe("s3cret");
  });

  it("sends a null row for a session that never got one", async () => {
    fetchWithTimeout.mockResolvedValue(rotated());
    await refreshTokens("rt_old", null);
    expect(sent().body).toEqual({ refresh_token: "rt_old", session_row_id: null });
  });

  // A 401 is the grant refused — a row revoked from another device among the
  // reasons — and a refusal is what ends the session at every caller.
  it("answers a refusal when auth refuses the grant", async () => {
    fetchWithTimeout.mockResolvedValue(new Response("{}", { status: 401 }));
    await expect(refreshTokens("rt_old", "sess_1")).resolves.toEqual({ kind: "refused" });
  });

  // No answer says nothing of the session, so the caller keeps it and asks
  // again, rather than signing somebody out because auth blinked.
  it("answers that auth was unreachable for a 5xx, a 429 and a request that failed", async () => {
    fetchWithTimeout.mockResolvedValueOnce(new Response("{}", { status: 503 }));
    await expect(refreshTokens("rt_old", null)).resolves.toEqual({ kind: "unreachable" });

    fetchWithTimeout.mockResolvedValueOnce(new Response("{}", { status: 429 }));
    await expect(refreshTokens("rt_old", null)).resolves.toEqual({ kind: "unreachable" });

    fetchWithTimeout.mockRejectedValueOnce(new Error("connect ECONNREFUSED"));
    await expect(refreshTokens("rt_old", null)).resolves.toEqual({ kind: "unreachable" });
  });

  // Auth rotated and its answer came cut off: it takes the spent token back
  // for thirty seconds, so the next ask inside them is answered — never a
  // sign-out for a body lost on its way.
  it("answers that auth was unreachable for a grant whose body did not arrive whole", async () => {
    fetchWithTimeout.mockResolvedValueOnce(new Response('{"accessToken":"at_', { status: 200 }));
    await expect(refreshTokens("rt_old", null)).resolves.toEqual({ kind: "unreachable" });
  });

  // The bearer is opaque, so the session id rides the answer or nothing
  // could name the session at sign-out.
  it("answers a refusal for a grant that names no session", async () => {
    fetchWithTimeout.mockResolvedValue(
      ok({
        userId: "user_1",
        email: "ada@example.test",
        emailVerified: true,
        firstName: "Ada",
        lastName: null,
        accessToken: "at_new",
        refreshToken: "rt_new",
        expiresIn: 3600,
      }),
    );
    await expect(refreshTokens("rt_old", "sess_1")).resolves.toEqual({ kind: "refused" });
  });
});

// The session is auth's own, whichever way it began: ending it is one call,
// and the browser's redirect through an external provider's page ends only
// that provider's.
describe("revokeSession", () => {
  it("asks auth to end the session", async () => {
    fetchWithTimeout.mockResolvedValue(new Response(null, { status: 204 }));
    await revokeSession("sid_1");
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/v1/auth/logout");
    expect(JSON.parse(init.body)).toEqual({ session_id: "sid_1" });
  });
});

// Whose code the callback spends is what the pending sign-in recorded: the
// external provider's when "Continue with…" sent the browser there, else
// auth's own.
describe("exchangeCode", () => {
  const signedIn = () =>
    ok({
      userId: "user_1",
      email: "ada@example.test",
      emailVerified: true,
      firstName: "Ada",
      lastName: null,
      accessToken: "at_1",
      sessionId: "ses_1",
      refreshToken: "rt_1",
      expiresIn: 900,
      idToken: "id.token.x",
      authMethod: null,
    });

  it("names the external provider when told, and nothing otherwise", async () => {
    fetchWithTimeout.mockResolvedValue(signedIn());
    const result = await exchangeCode("c0de", SPENT, "external");
    expect(result.idToken).toBe("id.token.x");
    expect(result.sessionId).toBe("ses_1");
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/v1/auth/exchange");
    expect(JSON.parse(init.body)).toEqual({
      code: "c0de",
      codeVerifier: "verifier_1",
      nonce: "nonce_1",
      provider: "external",
    });

    fetchWithTimeout.mockResolvedValue(signedIn());
    await exchangeCode("c0de", SPENT);
    const [, local] = fetchWithTimeout.mock.calls[1]!;
    expect(JSON.parse(local.body)).toEqual({
      code: "c0de",
      codeVerifier: "verifier_1",
      nonce: "nonce_1",
    });
  });

  // A refusal keeps its status and its problem's type, which the callback
  // turns into the reason the person reads.
  it("throws a refusal with its status and type", async () => {
    fetchWithTimeout.mockResolvedValue(
      new Response(JSON.stringify({ type: "/errors/auth/sign-up-closed", title: "sign-ups closed", status: 403 }), {
        status: 403,
        headers: { "content-type": "application/problem+json" },
      }),
    );
    const refused = await exchangeCode("c0de", SPENT, "external").catch((err: unknown) => err);
    expect(refused).toBeInstanceOf(ExchangeRefused);
    expect(refused).toMatchObject({ status: 403, type: "/errors/auth/sign-up-closed" });
  });

  // The challenge and the nonce ride with the state, so the code is bound to this sign-in.
  it("hands auth the challenge and the nonce with the state", async () => {
    fetchWithTimeout.mockResolvedValue(ok({ authorizeUrl: "https://idp.example/authorize" }));
    await getAuthorizeUrl("state_1", { ...PROOF, signUp: true });
    const [, init] = fetchWithTimeout.mock.calls[0]!;
    expect(JSON.parse(init.body)).toEqual({
      state: "state_1",
      codeChallenge: "challenge_1",
      nonce: "nonce_1",
      signUp: true,
    });
  });
});

// What the sign-in pages show, and the form alone when auth cannot say.
describe("getSignInConfig", () => {
  it("answers auth's configuration", async () => {
    fetchWithTimeout.mockResolvedValue(
      ok({ passwordSignIn: true, allowSignUp: false, verifyEmail: true, external: { name: "Okta" } }),
    );
    await expect(getSignInConfig()).resolves.toEqual({
      passwordSignIn: true,
      allowSignUp: false,
      verifyEmail: true,
      external: { name: "Okta" },
    });
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://auth.test/internal/v1/auth/config");
    expect(init.method).toBe("GET");
  });

  it("answers null when auth is down or answers nonsense", async () => {
    fetchWithTimeout.mockRejectedValue(new Error("ECONNREFUSED"));
    await expect(getSignInConfig()).resolves.toBeNull();
    fetchWithTimeout.mockResolvedValue(ok({ passwordSignIn: "yes" }));
    await expect(getSignInConfig()).resolves.toBeNull();
  });
});
