// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";

import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("notFound");
  },
}));

vi.mock("./_password", () => ({
  PasswordReset: ({ method }: { method: string | null }) => (
    <div data-testid="password">{method ?? "unknown"}</div>
  ),
}));

vi.mock("./_email", () => ({
  ChangeEmail: ({
    method,
    email,
  }: {
    method: string | null;
    email: string;
  }) => <div data-testid="email">{`${method ?? "unknown"}:${email}`}</div>,
}));

const session = vi.hoisted(() => ({
  value: {
    userId: "user_1",
    email: "k@example.com",
    firstName: "K",
    lastName: null,
    sessionRowId: null,
    authMethod: "google" as string | null,
  },
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => session.value,
}));

beforeEach(() => {
  session.value = { ...session.value, authMethod: "google" };
});

import AccountSettingsPage from "./page";

describe("AccountSettingsPage", () => {
  it("shows who you are", async () => {
    render(await AccountSettingsPage());
    expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
    expect(screen.getByText("k@example.com")).toBeInTheDocument();
    expect(screen.getByText("user_1")).toBeInTheDocument();
  });

  it("names how you signed in", async () => {
    render(await AccountSettingsPage());
    expect(screen.getByText("Sign-in method")).toBeInTheDocument();
    expect(screen.getByText("Google")).toBeInTheDocument();
  });

  it("draws no row at all when the provider said nothing", async () => {
    session.value = { ...session.value, authMethod: null };
    render(await AccountSettingsPage());
    expect(screen.queryByText("Sign-in method")).toBeNull();
    // The closed default has to survive the PAGE, not only the component. A
    // null method may well be a provider sign-up, so it gets what one gets:
    // nothing offered, rather than a control on a guess.
    expect(screen.queryByTestId("password")).toBeNull();
    expect(screen.queryByTestId("email")).toBeNull();
  });

  // ⚠ The pin for the whole rule. A Google account is told how it signs in and
  // offered nothing else — no Email section, no Password section, and no card
  // explaining the absence of either. Restoring the prose means deleting this.
  it("offers a provider account neither control, and no prose about it", async () => {
    render(await AccountSettingsPage());
    expect(screen.queryByTestId("email")).toBeNull();
    expect(screen.queryByTestId("password")).toBeNull();
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    expect(headings).toEqual(["profile"]);
    // The Profile row is the breadcrumb that replaced both paragraphs.
    expect(screen.getByText("Google")).toBeInTheDocument();
  });

  it("offers the password control to an email-and-password account", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountSettingsPage());
    expect(screen.getByTestId("password")).toHaveTextContent("password");
  });

  // Hands over the RAW method and the address from the cookie — not a label,
  // not a boolean. The page decides whether to DRAW the section, because only
  // it can remove the `<Section>` wrapper; what a method means past that is
  // `lib/sign-in-method`'s vocabulary, and the component and the Server Action
  // each ask it themselves.
  it("hands the email control the method and address it was given", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountSettingsPage());
    expect(screen.getByTestId("email")).toHaveTextContent(
      "password:k@example.com",
    );
  });

  it("offers no way to edit the name the provider owns", async () => {
    render(await AccountSettingsPage());
    // Scoped to Profile on purpose. The email control is stubbed here, so a
    // page-wide textbox count is vacuously zero and would protect nothing once
    // this page has a form on it at all.
    const profile = screen
      .getByRole("heading", { level: 2, name: /^profile$/i })
      .closest("section");
    expect(profile).not.toBeNull();
    expect(profile?.querySelectorAll("input, textarea")).toHaveLength(0);
  });

  it("holds who you are — no sessions, no erasure, no organization", async () => {
    session.value = { ...session.value, authMethod: "password" };
    render(await AccountSettingsPage());
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent?.toLowerCase() ?? "");
    // Order is the assertion, not just membership: who you are, then the two
    // halves of how you get in, with the address before the password that hangs
    // off it.
    expect(headings).toEqual(["profile", "email", "password"]);
    expect(screen.queryByText(/danger zone/i)).toBeNull();
    expect(screen.queryByText(/active sessions/i)).toBeNull();
  });

  it("fetches nothing", () => {
    const source = readFileSync(path.join(__dirname, "page.tsx"), "utf8");
    expect(source).not.toContain("@/lib/server/data");
  });
});
