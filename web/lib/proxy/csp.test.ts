import { afterEach, describe, expect, it } from "vitest";

import { contentSecurityPolicy } from "@/proxy";

const formAction = () =>
  contentSecurityPolicy("test-nonce")
    .split(";")
    .map((d) => d.trim())
    .find((d) => d.startsWith("form-action"));

describe("contentSecurityPolicy", () => {
  afterEach(() => {
    delete process.env.AUTH_PROVIDER_ORIGINS;
  });

  it("lets the sign-out redirect chain through form-action", () => {
    process.env.AUTH_PROVIDER_ORIGINS = "https://idp.example https://*.idp.example";
    const directive = formAction();
    expect(directive).toContain("'self'");
    expect(directive).toContain("https://idp.example");
    expect(directive).toContain("https://*.idp.example");
  });

  it("names each form target instead of retreating to a wildcard", () => {
    process.env.AUTH_PROVIDER_ORIGINS = "  https://idp.example\thttps://*.idp.example  ";
    expect(formAction()).toBe(
      "form-action 'self' https://idp.example https://*.idp.example",
    );
  });

  it("allows only this site when no provider origin is configured", () => {
    expect(formAction()).toBe("form-action 'self'");
  });
});
