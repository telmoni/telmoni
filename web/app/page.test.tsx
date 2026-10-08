// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { DOCS_URL, SKILLS_URL } from "@/lib/site";

let mockSession: { userId: string } | null = null;

vi.mock("@/components/paper-shell", () => ({
  PaperShell: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));
vi.mock("@/components/json-ld", () => ({ JsonLd: () => null }));
vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => mockSession,
}));

import SplashPage from "./page";

beforeEach(() => {
  mockSession = null;
});

describe("the splash page", () => {
  it("makes its claim as the one heading, and sends a visitor to sign up or to the docs", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(/stall, loop or overspend/);
    expect(screen.getAllByRole("link", { name: "Sign up" })[0]).toHaveAttribute(
      "href",
      "/auth/signup",
    );
    expect(screen.queryByRole("link", { name: "Console" })).toBeNull();
    expect(screen.getAllByRole("link", { name: "Documentation" })[0]).toHaveAttribute(
      "href",
      DOCS_URL,
    );
    expect(screen.getByText(/has not launched/)).toBeInTheDocument();
  });

  it("sends a member to the console instead", async () => {
    mockSession = { userId: "user_1" };

    render(await SplashPage());

    expect(screen.getAllByRole("link", { name: "Console" })[0]).toHaveAttribute(
      "href",
      "/console",
    );
    expect(screen.queryByRole("link", { name: "Sign up" })).toBeNull();
  });

  // A link that replaces the page you are reading with somebody else's site
  // has taken the back button with it; and a link into the console goes only
  // to the book, or where the proxy answers without a page file: sign-up and
  // the console.
  it("opens every link out of the console in a new tab, and names only the doors in", async () => {
    render(await SplashPage());

    for (const link of screen.getAllByRole("link")) {
      const href = link.getAttribute("href") ?? "";
      if (/^https?:/.test(href)) {
        expect(link, `${href} would replace the page`).toHaveAttribute("target", "_blank");
      } else {
        expect(
          href === "/auth/signup" || href === "/console" || href === DOCS_URL || href.startsWith(`${DOCS_URL}/`),
          `${href} is no door of the console's`,
        ).toBe(true);
      }
    }
  });

  it("puts the console agent beside the skill, the CLI and the API, each with its door", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: /send an agent/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Read the documentation/ })).toHaveAttribute(
      "href",
      `${DOCS_URL}/self-host/agent`,
    );
    expect(screen.getByRole("link", { name: /Install the skill/ })).toHaveAttribute(
      "href",
      SKILLS_URL ?? "",
    );
    expect(screen.getByRole("link", { name: /Install the CLI/ })).toHaveAttribute(
      "href",
      `${DOCS_URL}/api/cli`,
    );
    expect(screen.getByRole("link", { name: /Read the reference/ })).toHaveAttribute(
      "href",
      `${DOCS_URL}/api/reference`,
    );
  });

  it("answers the questions a visitor asks before signing up", async () => {
    render(await SplashPage());

    for (const question of [
      "What is Telmoni?",
      "Has it launched?",
      "What does it store?",
      "Can I self-host it?",
      "How do I get started?",
    ]) {
      expect(screen.getByText(question)).toBeInTheDocument();
    }
  });
});
