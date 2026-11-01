// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("NEXT_NOT_FOUND");
  },
}));
vi.mock("next/server", async (importOriginal) => ({
  ...(await importOriginal<typeof import("next/server")>()),
  connection: async () => {},
}));

let mockConfig: unknown;
vi.mock("@/lib/auth/oidc", () => ({
  getSignInConfig: async () => mockConfig,
}));

vi.mock("../_accounts/actions", () => ({ forgotPasswordAction: vi.fn() }));

import ForgotPage from "./page";

const CONFIG = { allowSignUp: false, verifyEmail: false, external: null };

beforeEach(() => {
  mockConfig = { ...CONFIG, passwordSignIn: true };
});

describe("the forgotten password page", () => {
  it("asks for the address while the password form is on, and while auth cannot say", async () => {
    render(await ForgotPage());
    expect(screen.getByText("Reset your password")).toBeInTheDocument();
    mockConfig = null;
    render(await ForgotPage());
    expect(screen.getAllByText("Reset your password")).toHaveLength(2);
  });

  // Its lane is not mounted with the form off, so a deployment on a provider
  // alone, the hosted service among them, has no such page.
  it("is not found while the password form is off", async () => {
    mockConfig = { ...CONFIG, passwordSignIn: false };
    await expect(ForgotPage()).rejects.toThrow("NEXT_NOT_FOUND");
  });
});
