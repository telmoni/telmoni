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

vi.mock("../_accounts/actions", () => ({ resetPasswordAction: vi.fn() }));

import ResetPage from "./page";

const CONFIG = { allowSignUp: false, verifyEmail: false, external: null };
const LINK = { searchParams: Promise.resolve({ uid: "user_1", token: "t0ken" }) };

beforeEach(() => {
  mockConfig = { ...CONFIG, passwordSignIn: true };
});

describe("the new password page", () => {
  it("asks for the new password while the password form is on", async () => {
    render(await ResetPage(LINK as never));
    expect(screen.getByText("Choose a new password")).toBeInTheDocument();
  });

  // As the page that mails the link: none with the form off, whatever the link holds.
  it("is not found while the password form is off", async () => {
    mockConfig = { ...CONFIG, passwordSignIn: false };
    await expect(ResetPage(LINK as never)).rejects.toThrow("NEXT_NOT_FOUND");
  });
});
