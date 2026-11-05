import { afterEach, describe, expect, it, vi } from "vitest";

import { env, isSecureOrigin } from "./env";

describe("isSecureOrigin", () => {
  it("accepts any https origin", () => {
    for (const origin of [
      "https://app.example.com",
      "https://app.example.com:8443",
      "https://localhost:3000",
    ]) {
      expect(isSecureOrigin(origin), origin).toBe(true);
    }
  });

  it("accepts http on loopback, whatever the port", () => {
    for (const origin of [
      "http://localhost:3000",
      "http://localhost",
      "http://127.0.0.1:3000",
      "http://[::1]:3000",
    ]) {
      expect(isSecureOrigin(origin), origin).toBe(true);
    }
  });

  it("rejects http on any host that is not loopback", () => {
    for (const origin of [
      "http://app.example.com",
      "http://10.0.0.5:3000",
      "http://0.0.0.0:3000",
    ]) {
      expect(isSecureOrigin(origin), origin).toBe(false);
    }
  });

  it("rejects hosts that merely begin with a loopback name", () => {
    for (const origin of [
      "http://localhost.attacker.example",
      "http://localhost-evil.example",
      "http://127.0.0.1.attacker.example",
    ]) {
      expect(isSecureOrigin(origin), origin).toBe(false);
    }
  });

  it("rejects a non-http scheme and an unparseable value", () => {
    for (const origin of ["ftp://localhost", "file:///etc/passwd", "localhost:3000", ""]) {
      expect(isSecureOrigin(origin), origin).toBe(false);
    }
  });
});

describe("SEARCH_INDEXING", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("offers the public pages to the engines on `true` alone", () => {
    for (const raw of ["true", " true "]) {
      vi.stubEnv("SEARCH_INDEXING", raw);
      expect(env.SEARCH_INDEXING, raw).toBe(true);
    }
  });

  // A spelling the console does not read keeps the deployment out of every index.
  it("keeps them out on anything else, unset included", () => {
    for (const raw of ["", "false", "TRUE", "1", "yes"]) {
      vi.stubEnv("SEARCH_INDEXING", raw);
      expect(env.SEARCH_INDEXING, raw).toBe(false);
    }
    vi.stubEnv("SEARCH_INDEXING", undefined);
    expect(env.SEARCH_INDEXING).toBe(false);
  });
});

describe("the deployment's optional settings", () => {
  const saved = { ...process.env };
  afterEach(() => {
    process.env = { ...saved };
  });

  it("lists the sign-in providers' origins as the tokens given, however spaced", () => {
    process.env.AUTH_PROVIDER_ORIGINS = "  https://a.example.com\nhttps://b.example.com  ";
    expect(env.AUTH_PROVIDER_ORIGINS).toEqual(["https://a.example.com", "https://b.example.com"]);
    delete process.env.AUTH_PROVIDER_ORIGINS;
    expect(env.AUTH_PROVIDER_ORIGINS).toEqual([]);
  });

  it("reads the announcement trimmed, and a blank one as none", () => {
    process.env.TELMONI_ANNOUNCEMENT = "  Maintenance at 02:00 UTC ";
    expect(env.TELMONI_ANNOUNCEMENT).toBe("Maintenance at 02:00 UTC");
    process.env.TELMONI_ANNOUNCEMENT = "   ";
    expect(env.TELMONI_ANNOUNCEMENT).toBeUndefined();
  });
});
