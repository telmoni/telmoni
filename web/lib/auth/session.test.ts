import { describe, it, expect, beforeEach } from "vitest";
import {
  sealConnect,
  sealPkce,
  unsealConnect,
  unsealPkce,
  sealSession,
  type SessionData,
} from "./session";

beforeEach(() => {
  process.env.AUTH_SECRET = "test-secret-that-is-32-chars-long!!";
});

const pkcePayload = {
  state: "random-state-xyz",
  returnTo: "/runs",
};

const sessionPayload: SessionData = {
  userId: "user_01",
  email: "test@example.com",
  firstName: "Test",
  lastName: "User",
  accessToken: "tok_abc",
  refreshToken: null,
  sessionId: "ses_abc",
  expiresAt: 0,
  idToken: "id_tok_xyz",
  authMethod: "password",
  sessionRowId: null,
};

describe("sealPkce / unsealPkce", () => {
  it("round-trips PKCE state through seal/unseal", async () => {
    const sealed = await sealPkce(pkcePayload);
    expect(typeof sealed).toBe("string");
    expect(sealed.length).toBeGreaterThan(20);

    const unsealed = await unsealPkce(sealed);
    expect(unsealed).toEqual(pkcePayload);
  });

  it("returns null when the sealed value is garbage", async () => {
    const result = await unsealPkce("not-a-valid-seal");
    expect(result).toBeNull();
  });

  it("returns null when AUTH_SECRET is wrong", async () => {
    const sealed = await sealPkce(pkcePayload);
    process.env.AUTH_SECRET = "completely-different-secret-here!!";
    const result = await unsealPkce(sealed);
    expect(result).toBeNull();
  });

  it("returns null for empty string input", async () => {
    const result = await unsealPkce("");
    expect(result).toBeNull();
  });
});

describe("sealConnect / unsealConnect", () => {
  const pending = {
    state: "st8",
    organizationId: "org_abc",
    projectId: "project_abc",
    provider: "slack" as const,
  };

  it("round-trips the handshake through seal/unseal", async () => {
    const sealed = await sealConnect(pending);
    expect(await unsealConnect(sealed)).toEqual(pending);
  });

  it("refuses garbage, an empty value and a wrong secret", async () => {
    expect(await unsealConnect("not-a-valid-seal")).toBeNull();
    expect(await unsealConnect("")).toBeNull();
    const sealed = await sealConnect(pending);
    process.env.AUTH_SECRET = "completely-different-secret-here!!";
    expect(await unsealConnect(sealed)).toBeNull();
  });

  it("refuses a sealed PKCE cookie presented as a handshake", async () => {
    // Both are sealed under one secret; only the shape tells them apart, and
    // a callback that accepted a PKCE payload would have no project to act on.
    const sealed = await sealPkce({ state: "st8", returnTo: "/console" });
    expect(await unsealConnect(sealed)).toBeNull();
  });
});

describe("sealSession", () => {
  it("produces a non-empty sealed string", async () => {
    const sealed = await sealSession(sessionPayload);
    expect(typeof sealed).toBe("string");
    expect(sealed.length).toBeGreaterThan(50);
  });

  it("different payloads produce different seals", async () => {
    const a = await sealSession({ ...sessionPayload, userId: "user_A" });
    const b = await sealSession({ ...sessionPayload, userId: "user_B" });
    expect(a).not.toBe(b);
  });

  it("same payload + same secret produces deterministic output... or at least round-trippable output", async () => {
    const { unsealData } = await import("iron-session");
    const sealed = await sealSession(sessionPayload);
    const recovered = await unsealData<SessionData>(sealed, {
      password: process.env.AUTH_SECRET!,
    });
    expect(recovered.userId).toBe(sessionPayload.userId);
    expect(recovered.email).toBe(sessionPayload.email);
  });
});
