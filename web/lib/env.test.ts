import { describe, expect, it } from "vitest";

import { isSecureOrigin } from "./env";

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
