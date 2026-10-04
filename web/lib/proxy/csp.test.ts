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

  // A token that is not an origin would reach the browser as a source
  // expression: `*` opens the directive to every site, a bare word or a
  // path breaks it, and either is an operator's typo rather than a decision.
  it("drops anything that is not an origin", () => {
    process.env.AUTH_PROVIDER_ORIGINS =
      "* idp.example https://idp.example/login https://idp.example:8443 javascript:alert(1)";
    expect(formAction()).toBe("form-action 'self' https://idp.example:8443");
  });

  // An operator's spelling of the provider — a default port, a trailing
  // slash, capitals — is the same origin to the browser, and the directive
  // gets it as the browser spells it rather than losing the sign-out to it.
  it("spells each origin as the browser does", () => {
    process.env.AUTH_PROVIDER_ORIGINS = "https://IdP.example:443/ https://*.idp.example/";
    expect(formAction()).toBe("form-action 'self' https://idp.example https://*.idp.example");
  });
});
