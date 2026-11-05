// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { DOCS_URL } from "@/lib/site";

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
  window.localStorage.clear();
  window.sessionStorage.clear();
});

describe("the splash page", () => {
  describe("where the console's trails are kept", () => {
    function keepTrails() {
      window.localStorage.setItem(
        "telmoni-recent-visits",
        JSON.stringify({ owner: "user_1", visits: [{ href: "/acme", label: "Acme", at: 0 }] }),
      );
      window.sessionStorage.setItem("telmoni-console-trail", JSON.stringify(["/acme"]));
      window.localStorage.setItem("theme", "dark");
    }

    it("forgets them for a signed-out visitor, where every sign-out lands, and keeps the preferences", async () => {
      keepTrails();

      render(await SplashPage());

      expect(window.localStorage.getItem("telmoni-recent-visits")).toBeNull();
      expect(window.sessionStorage.getItem("telmoni-console-trail")).toBeNull();
      expect(window.localStorage.getItem("theme")).toBe("dark");
    });

    it("leaves a member's alone", async () => {
      mockSession = { userId: "user_1" };
      keepTrails();

      render(await SplashPage());

      expect(window.localStorage.getItem("telmoni-recent-visits")).not.toBeNull();
      expect(window.sessionStorage.getItem("telmoni-console-trail")).not.toBeNull();
    });
  });

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
    expect(screen.queryByText(/not launched/i)).toBeNull();
  });

  it("says the product is in beta, beside what it is", async () => {
    render(await SplashPage());

    expect(screen.getByText("Beta")).toBeInTheDocument();
    expect(screen.getByText("Open-source monitoring for AI agents")).toBeInTheDocument();
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

  it("says what it catches, each with the page of the book that does it", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: "What it catches" })).toBeInTheDocument();
    for (const [label, path] of [
      ["Loops and stalls", "/monitoring/alerts#loops-and-stalls"],
      ["Budgets", "/monitoring/alerts#a-budget"],
      ["The guard", "/api/sdks#the-guard"],
    ]) {
      expect(screen.getByRole("link", { name: label })).toHaveAttribute("href", `${DOCS_URL}${path}`);
    }
  });

  it("offers the Claude Code plugin beside the SDKs, and nothing the page no longer sells", async () => {
    render(await SplashPage());

    expect(screen.getByRole("tab", { name: "claude code" })).toBeInTheDocument();
    for (const gone of ["What is Telmoni?", "Why Telmoni?", "Start in five minutes"]) {
      expect(screen.queryByRole("heading", { name: gone })).toBeNull();
    }
    expect(screen.queryByText(/Evaluators/)).toBeNull();
  });

  it("answers the questions a visitor asks before signing up", async () => {
    render(await SplashPage());

    for (const question of [
      "How do I get started?",
      "What does it store?",
      "Can I self-host it?",
      "Where do alerts go?",
    ]) {
      expect(screen.getByText(question)).toBeInTheDocument();
    }
  });

  it("names the stack it works with, each chip a page of the book", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: "Works with your stack" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "LangChain and LangGraph" })).toHaveAttribute(
      "href",
      `${DOCS_URL}/api/frameworks#langchain-and-langgraph`,
    );
  });

  // A key is answered and announced, never drawn on a button.
  it("draws no key on its buttons", async () => {
    render(await SplashPage());

    expect(screen.getAllByRole("link", { name: "Sign up" }).at(-1)).toHaveTextContent(/^Sign up$/);
    expect(screen.getAllByRole("link", { name: "Documentation" }).at(-1)).toHaveTextContent(/^Documentation$/);
  });

  it("no longer says no prompt is ever kept, now that a project may keep content", async () => {
    render(await SplashPage());

    expect(screen.queryByText(/single prompt/)).toBeNull();
    expect(screen.queryByText(/only mode is off/)).toBeNull();
  });
});
