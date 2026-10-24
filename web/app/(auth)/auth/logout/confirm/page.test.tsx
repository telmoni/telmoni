// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mockRedirect = vi.fn();
vi.mock("next/navigation", () => ({
  redirect: (url: string) => mockRedirect(url),
}));

let mockSession: { userId: string } | null;
vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => mockSession,
}));

import ConfirmSignOutPage from "./page";

beforeEach(() => {
  mockRedirect.mockReset();
  mockSession = { userId: "user_test" };
});

describe("the sign-out confirmation", () => {
  // A link is all it takes to reach `/auth/logout`, so the sign-out is a post.
  it("asks a live session, the sign-out a post and the way back a link", async () => {
    render(await ConfirmSignOutPage());

    const form = screen.getByRole("button", { name: "Sign out" }).closest("form");
    expect(form).toHaveAttribute("action", "/auth/logout");
    expect(form).toHaveAttribute("method", "post");
    expect(screen.getByRole("link", { name: "Stay signed in" })).toHaveAttribute("href", "/console");
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  it("sends a visitor with no session home", async () => {
    mockSession = null;
    await ConfirmSignOutPage();
    expect(mockRedirect).toHaveBeenCalledWith("/");
  });
});
