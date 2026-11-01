// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("NEXT_NOT_FOUND");
  },
}));

let mockConfig: unknown;
vi.mock("@/lib/auth/oidc", () => ({
  getSignInConfig: async () => mockConfig,
}));

// A sign-in in progress, as the door seals it on the provider's path too.
vi.mock("../_accounts/pending", () => ({
  pendingSignIn: async () => ({ state: "st_1", verifier: "v" }),
}));
vi.mock("./_form", () => ({ SignUpForm: () => <div>the form</div> }));

import SignUpPage from "./page";

const CONFIG = { allowSignUp: true, verifyEmail: false, external: null };
const page = () => SignUpPage({ searchParams: Promise.resolve({ state: "st_1" }), params: Promise.resolve({}) });

beforeEach(() => {
  mockConfig = { ...CONFIG, passwordSignIn: true };
});

describe("the sign-up page", () => {
  it("draws the form while the password form is on, and while auth cannot say", async () => {
    render(await page());
    expect(screen.getByText("Create an account")).toBeInTheDocument();
    expect(screen.getByText("the form")).toBeInTheDocument();
    mockConfig = null;
    render(await page());
    expect(screen.getAllByText("Create an account")).toHaveLength(2);
  });

  // Its lane is not mounted with the form off, so a deployment on a provider
  // alone, the hosted service among them, has no such page, whatever state
  // the door sealed for the sign-in in progress.
  it("is not found while the password form is off", async () => {
    mockConfig = { ...CONFIG, passwordSignIn: false };
    await expect(page()).rejects.toThrow("NEXT_NOT_FOUND");
  });
});
