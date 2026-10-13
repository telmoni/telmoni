import { afterEach, describe, expect, it, vi } from "vitest";

import { env, isSecureOrigin } from "./env";
import { LIVE_HOLD_SECS } from "./live-refresh";

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

describe("LIVE_REFRESH_HOLD_SECS", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("is the hold the listener ships with when unset or blank", () => {
    vi.stubEnv("LIVE_REFRESH_HOLD_SECS", "");
    expect(env.LIVE_REFRESH_HOLD_SECS).toBe(LIVE_HOLD_SECS);
    vi.stubEnv("LIVE_REFRESH_HOLD_SECS", "  ");
    expect(env.LIVE_REFRESH_HOLD_SECS).toBe(LIVE_HOLD_SECS);
  });

  it("reads whole seconds, zero included", () => {
    vi.stubEnv("LIVE_REFRESH_HOLD_SECS", "0");
    expect(env.LIVE_REFRESH_HOLD_SECS).toBe(0);
    vi.stubEnv("LIVE_REFRESH_HOLD_SECS", " 120 ");
    expect(env.LIVE_REFRESH_HOLD_SECS).toBe(120);
    vi.stubEnv("LIVE_REFRESH_HOLD_SECS", "3600");
    expect(env.LIVE_REFRESH_HOLD_SECS).toBe(3600);
  });

  it("stops the console on anything that is not a whole number of seconds up to an hour", () => {
    for (const raw of ["3601", "-1", "1.5", "30s", "abc"]) {
      vi.stubEnv("LIVE_REFRESH_HOLD_SECS", raw);
      expect(() => env.LIVE_REFRESH_HOLD_SECS, raw).toThrow(/LIVE_REFRESH_HOLD_SECS/);
    }
  });
});
