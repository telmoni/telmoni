import { beforeEach, describe, expect, it, vi } from "vitest";

const jar = vi.hoisted(() => ({ value: undefined as string | undefined }));
vi.mock("next/headers", () => ({
  cookies: async () => ({ get: (name: string) => (name === "telmoni_pkce" && jar.value ? { value: jar.value } : undefined) }),
}));
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Error(`redirect:${to}`);
  },
}));
vi.mock("@/lib/auth/session", () => ({
  PKCE_COOKIE: "telmoni_pkce",
  unsealPkce: async (sealed: string) =>
    sealed.startsWith("sealed:") ? JSON.parse(sealed.slice("sealed:".length)) : null,
}));

import { one, pendingSignIn } from "./pending";

describe("pendingSignIn", () => {
  beforeEach(() => {
    jar.value = "sealed:" + JSON.stringify({ state: "st8", returnTo: "/console" });
  });

  it("hands the page the sign-in the door started, when the state agrees", async () => {
    expect(await pendingSignIn("st8")).toEqual({ state: "st8", returnTo: "/console" });
  });

  // ⚠ The gate is what keeps these pages from being a second, unguarded way
  // in: without a live cookie the form would post a password to a lane that
  // may not exist, and the callback would refuse the code anyway.
  it.each([
    ["no state in the query", undefined],
    ["another state", "someone-elses"],
  ])("sends a visit with %s back through /auth/login", async (_why, state) => {
    await expect(pendingSignIn(state)).rejects.toThrow("redirect:/auth/login");
  });

  it("sends a visit with no cookie, or an unreadable one, back through /auth/login", async () => {
    jar.value = undefined;
    await expect(pendingSignIn("st8")).rejects.toThrow("redirect:/auth/login");
    jar.value = "garbage";
    await expect(pendingSignIn("st8")).rejects.toThrow("redirect:/auth/login");
  });
});

describe("one", () => {
  it("takes one string and nothing else", () => {
    expect(one("x")).toBe("x");
    expect(one("")).toBeUndefined();
    expect(one(["x", "y"])).toBeUndefined();
    expect(one(undefined)).toBeUndefined();
  });
});
