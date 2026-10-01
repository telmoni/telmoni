// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const revoke = vi.hoisted(() => vi.fn(async () => ({ error: null })));
vi.mock("./actions", () => ({ revokeSessionAction: revoke }));
const refresh = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({ useRouter: () => ({ refresh }) }));
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: toastSuccess, error: toastError }),
}));

import { ActiveSessions, deviceLabel } from "./_sessions";
import type { ActiveSession } from "@/lib/server/data";

const CHROME_MAC =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
const SAFARI_IOS =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
const EDGE_WIN =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 Edg/120.0";

const rows: ActiveSession[] = [
  {
    id: "s-current",
    user_agent: CHROME_MAC,
    created_at: "2026-08-26T14:00:00.000Z",
    last_seen_at: "2026-08-26T15:00:00.000Z",
  },
  {
    id: "s-other",
    user_agent: EDGE_WIN,
    created_at: "2026-08-20T09:00:00.000Z",
    last_seen_at: "2026-08-25T09:00:00.000Z",
  },
];

beforeEach(() => vi.clearAllMocks());

describe("deviceLabel", () => {
  it("picks the most specific browser, not the first one it matches", () => {
    expect(deviceLabel(EDGE_WIN)).toBe("Edge (Windows)");
    expect(deviceLabel(CHROME_MAC)).toBe("Chrome (macOS)");
    expect(deviceLabel(SAFARI_IOS)).toBe("Safari (iOS)");
  });

  it("says Unknown device rather than guessing", () => {
    expect(deviceLabel(null)).toBe("Unknown device");
    expect(deviceLabel("curl/8.4.0")).toBe("Unknown device");
  });

  it("names the CLI as a terminal, with the platform it runs on", () => {
    expect(deviceLabel("telmoni-cli/0.0.1 (macos; aarch64)")).toBe("Telmoni CLI (macOS)");
    expect(deviceLabel("telmoni-cli/0.2.0 (linux; x86_64)")).toBe("Telmoni CLI (Linux)");
    expect(deviceLabel("telmoni-cli/0.2.0 (windows; x86_64)")).toBe("Telmoni CLI (Windows)");
    expect(deviceLabel("telmoni-cli/0.2.0")).toBe("Telmoni CLI");
    expect(deviceLabel("telmoni-cli/0.2.0 (freebsd; x86_64)")).toBe("Telmoni CLI");
    expect(deviceLabel("Mozilla/5.0 telmoni-cli/1.0 Chrome/120.0")).toBe("Chrome");
  });
});

describe("ActiveSessions", () => {
  it("marks the caller's own row and no other", () => {
    render(<ActiveSessions sessions={rows} currentId="s-current" />);
    const current = screen.getAllByText("Current");
    expect(current).toHaveLength(1);
    expect(current[0]!.closest("tr")!.textContent).toContain("Chrome (macOS)");
  });

  it("marks nothing when the session predates the registry", () => {
    render(<ActiveSessions sessions={rows} currentId={null} />);
    expect(screen.queryByText("Current")).toBeNull();
  });

  it("offers no End action on the current session", () => {
    render(<ActiveSessions sessions={rows} currentId="s-current" />);
    const controls = screen.getAllByRole("button", { name: /End session/i });
    expect(controls).toHaveLength(1);
    expect(controls[0]!.getAttribute("aria-label")).toContain("Edge (Windows)");
  });

  it("ends the session whose row the control belongs to", async () => {
    render(<ActiveSessions sessions={rows} currentId="s-current" />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /End session/i }));
    });
    expect(revoke).toHaveBeenCalledWith("s-other");
  });

  it("tells the customer the sign-out is not instant", async () => {
    render(<ActiveSessions sessions={rows} currentId="s-current" />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /End session/i }));
    });
    expect(toastSuccess).toHaveBeenCalledWith(
      expect.stringMatching(/within a few minutes/i),
    );
  });

  it("says so when the list cannot be read", () => {
    render(<ActiveSessions sessions={null} currentId={null} />);
    expect(screen.getByText(/Couldn.t load your sessions/i)).toBeInTheDocument();
    expect(document.querySelector("table")).toBeNull();
  });
});
