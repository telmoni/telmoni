import { describe, expect, it } from "vitest";

import { isPublic } from "./public-paths";

describe("auth entry points", () => {
  it("keeps both sign-in and sign-up reachable while signed out", () => {
    expect(isPublic("/auth/login")).toBe(true);
    expect(isPublic("/auth/signup")).toBe(true);
  });

  it("keeps the invite landing behind the gate", () => {
    expect(isPublic("/invites/accept")).toBe(false);
  });
});
