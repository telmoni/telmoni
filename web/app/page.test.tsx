// @vitest-environment jsdom
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

  it("makes its claim as the one heading, and sends a visitor to sign up or to the SDK docs", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(
      /Privacy-first, open-source monitoring for AI agents/,
    );
    expect(screen.getAllByRole("link", { name: "Sign up" })[0]).toHaveAttribute(
      "href",
      "/auth/signup",
    );
    expect(screen.queryByRole("link", { name: "Console" })).toBeNull();
    expect(screen.getByRole("link", { name: "Read docs" })).toHaveAttribute(
      "href",
      `${DOCS_URL}/api/sdks`,
    );
    expect(screen.queryByText(/not launched/i)).toBeNull();
  });

  it("says what it is in the subtitle", async () => {
    render(await SplashPage());

    expect(screen.getByText(/stall, loop or overspend/)).toBeInTheDocument();
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
          href === "/auth/signup" ||
            href === "/console" ||
            href === DOCS_URL ||
            href.startsWith(`${DOCS_URL}/`),
          `${href} is no door of the console's`,
        ).toBe(true);
      }
    }
  });

  it("says what it catches, each with the page of the book that does it", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: "What is Telmoni?" })).toBeInTheDocument();
    for (const [label, path] of [
      ["Loops and stalls", "/monitoring/alerts#loops-and-stalls"],
      ["Budgets", "/monitoring/alerts#a-budget"],
      ["The guard", "/api/sdks#the-guard"],
    ]) {
      expect(screen.getByRole("link", { name: label })).toHaveAttribute(
        "href",
        `${DOCS_URL}${path}`,
      );
    }
  });

  it("offers the Console, Terminal (with curl and cargo install popover), and SDK surface cards", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: "Console", level: 3 })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Terminal", level: 3 })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "SDK", level: 3 })).toBeInTheDocument();

    const installBtn = screen.getByRole("button", { name: /Install/i });
    fireEvent.click(installBtn);

    expect(screen.getByText("curl -fsSL https://telmoni.com/install.sh | sh")).toBeInTheDocument();
    expect(screen.getByText("cargo install --locked telmoni-cli")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "Read docs" })).toHaveAttribute(
      "href",
      `${DOCS_URL}/api/sdks`,
    );
    for (const gone of [
      "Why Telmoni?",
      "Start in five minutes",
      "Questions and answers",
      "Everything you need to monitor your agents",
    ]) {
      expect(screen.queryByRole("heading", { name: gone })).toBeNull();
    }
    expect(screen.queryByText(/Evaluators/)).toBeNull();
  });

  it("copies the chosen install command and returns focus after Escape", async () => {
    const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    try {
      render(await SplashPage());
      const trigger = screen.getByRole("button", { name: "Install" });
      fireEvent.click(trigger);
      const dialog = screen.getByRole("dialog", { name: "Terminal install commands" });
      expect(dialog).toBeVisible();
      for (const [label, command] of [
        ["curl", "curl -fsSL https://telmoni.com/install.sh | sh"],
        ["cargo", "cargo install --locked telmoni-cli"],
      ]) {
        const button = screen.getByRole("button", { name: `Copy ${label} install command` });
        await act(async () => { fireEvent.click(button); });
        expect(writeText).toHaveBeenLastCalledWith(command);
        expect(button).toHaveTextContent("Copied");
      }
      fireEvent.keyDown(dialog, { key: "Escape" });
      expect(screen.queryByRole("dialog")).toBeNull();
      await waitFor(() => expect(trigger).toHaveFocus());
    } finally {
      if (original) Object.defineProperty(navigator, "clipboard", original);
      else Reflect.deleteProperty(navigator, "clipboard");
    }
  });

  it("presents the open source section with Apache-2.0 and contribute link", async () => {
    render(await SplashPage());

    expect(screen.getByRole("heading", { name: "Open source" })).toBeInTheDocument();
    expect(screen.getByText(/Apache-2.0 licensed/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Contribute/ })).toHaveAttribute(
      "href",
      `${DOCS_URL}/contributing/introduction`,
    );
  });

  // A key is answered and announced, never drawn on a button.
  it("draws no key on its buttons", async () => {
    render(await SplashPage());

    expect(screen.getAllByRole("link", { name: "Sign up" }).at(-1)).toHaveTextContent(/^Sign up$/);
    expect(screen.getByRole("link", { name: "Read docs" })).toHaveTextContent(/^Read docs$/);
  });

  it("says where prompts stay, and no longer that none is ever kept, now that a project may keep content", async () => {
    render(await SplashPage());

    expect(
      screen.getByText(/Prompts stay in your process unless you turn content on./),
    ).toBeInTheDocument();
    expect(screen.queryByText(/single prompt/)).toBeNull();
    expect(screen.queryByText(/only mode is off/)).toBeNull();
  });
});
