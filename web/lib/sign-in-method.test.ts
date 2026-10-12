import { describe, expect, it } from "vitest";

import {
  canChangeEmail,
  canResetPassword,
  emailPosture,
  passwordPosture,
  signInMethodLabel,
} from "./sign-in-method";

describe("signInMethodLabel", () => {
  it("names the password form, the one method the core's auth records", () => {
    expect(signInMethodLabel("password")).toBe("Email and password");
  });

  it("capitalises a method a deployment's own provider reports", () => {
    expect(signInMethodLabel("okta")).toBe("Okta");
  });

  it("does not answer for a key it never declared", () => {
    expect(signInMethodLabel("constructor")).toBe("Constructor");
    expect(signInMethodLabel("__proto__")).toBe("__proto__");
    expect(signInMethodLabel("toString")).toBe("ToString");
  });

  it("returns nothing to print when there is nothing to print", () => {
    expect(signInMethodLabel(null)).toBeNull();
    expect(signInMethodLabel(undefined)).toBeNull();
    expect(signInMethodLabel("")).toBeNull();
  });
});

describe("passwordPosture", () => {
  it("offers a reset only to the people who have a password", () => {
    expect(passwordPosture("password")).toBe("reset");
  });

  // Users authenticating via external providers do not have a local password reset.
  it("offers nothing to anybody whose provider holds the credential", () => {
    for (const m of [
      "google",
      "microsoft",
      "github",
      "passkey",
      "magic_link",
      "okta",
      "sso",
    ]) {
      expect(passwordPosture(m), m).toBe("provider");
    }
  });

  // A null method is a real state, and one of those people may be a provider
  // sign-up. Failing closed costs a sign-out; failing open offers a credential.
  it("offers nothing when the session does not say", () => {
    expect(passwordPosture(null)).toBe("unknown");
    expect(passwordPosture(undefined)).toBe("unknown");
    expect(passwordPosture("")).toBe("unknown");
  });

  it("claims nothing when the provider said nothing", () => {
    expect(passwordPosture(null)).toBe("unknown");
    expect(passwordPosture(undefined)).toBe("unknown");
  });
});

describe("canChangeEmail", () => {
  // The address belongs upstream for every other connection: a change made
  // here is overwritten by the next `/me`, or leaves this record disagreeing
  // with the identity provider.
  it("is only true for an email-and-password sign-up", () => {
    expect(canChangeEmail("password")).toBe(true);
    for (const m of [
      "google",
      "microsoft",
      "github",
      "passkey",
      "magic_link",
      "okta",
      "sso",
    ]) {
      expect(canChangeEmail(m), m).toBe(false);
    }
  });

  it("fails closed when the session does not say", () => {
    expect(canChangeEmail(null)).toBe(false);
    expect(canChangeEmail(undefined)).toBe(false);
  });
});

describe("emailPosture", () => {
  it("gives an email-and-password account the one posture with a form", () => {
    expect(emailPosture("password")).toBe("change");
  });

  it("gives every other connection the same nothing", () => {
    for (const m of [
      "google",
      "microsoft",
      "github",
      "passkey",
      "magic_link",
      "okta",
      "sso",
    ]) {
      expect(emailPosture(m), m).toBe("provider");
    }
  });

  it("fails closed when the session does not say", () => {
    expect(emailPosture(null)).toBe("unknown");
    expect(emailPosture(undefined)).toBe("unknown");
    expect(emailPosture("")).toBe("unknown");
  });

  // ⚠ These are two functions on purpose: moving an address and resetting a
  // password are different trades, and only the second mints a credential.
  // This pins that they agree TODAY without making either one read the
  // other's answer.
  it("agrees with passwordPosture today, while staying free to diverge", () => {
    const pairs: Array<[string | null, string, string]> = [
      ["password", "change", "reset"],
      ["sso", "provider", "provider"],
      ["google", "provider", "provider"],
      ["magic_link", "provider", "provider"],
      [null, "unknown", "unknown"],
    ];
    for (const [method, email, password] of pairs) {
      expect(emailPosture(method), String(method)).toBe(email);
      expect(passwordPosture(method), String(method)).toBe(password);
    }
  });
});

describe("canChangeEmail agrees with emailPosture", () => {
  // One question, one answer. `canChangeEmail` asks `emailPosture`, so the two
  // cannot drift apart into two different ideas of who is eligible.
  it("is emailPosture read as a boolean", () => {
    for (const m of [
      "password",
      "sso",
      "google",
      "magic_link",
      "okta",
      "",
      null,
      undefined,
    ]) {
      expect(canChangeEmail(m), String(m)).toBe(emailPosture(m) === "change");
    }
  });
});

describe("canResetPassword agrees with passwordPosture", () => {
  // The mirror of the pair above, and it earns its place for the same reason:
  // `page.tsx` decides whether to draw either section from these two booleans,
  // so a boolean that answered differently from the posture it claims to read
  // would draw a section the control inside it refuses to fill.
  it("is passwordPosture read as a boolean", () => {
    for (const m of [
      "password",
      "sso",
      "google",
      "magic_link",
      "okta",
      "",
      null,
      undefined,
    ]) {
      expect(canResetPassword(m), String(m)).toBe(
        passwordPosture(m) === "reset",
      );
    }
  });

  it("is true for exactly one method", () => {
    expect(canResetPassword("password")).toBe(true);
    for (const m of ["sso", "google", "microsoft", "passkey", "magic_link"]) {
      expect(canResetPassword(m), m).toBe(false);
    }
  });
});
