// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <header>
      <h1>{title}</h1>
      {action}
    </header>
  ),
}));
vi.mock("@/components/local-time", () => ({ LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span> }));
vi.mock("@/components/mark-read", () => ({
  MarkRead: ({ unread }: { unread: number }) => <button type="button">Mark read ({unread})</button>,
}));
vi.mock("../notice-actions", () => ({ markProjectReadAction: vi.fn() }));
vi.mock("@/lib/server/project-page", () => ({
  projectPage: async () => ({ kind: "ok", gate: {}, project: { id: "project_web", slug: "web", name: "Web" } }),
}));
let mockFeed: unknown;
vi.mock("@/lib/server/data", () => ({ fetchProjectNotifications: async () => mockFeed }));

import ProjectNotificationsPage from "./page";

const page = async () =>
  render(
    await ProjectNotificationsPage({
      params: Promise.resolve({ organization: "acme", project: "web" }),
      searchParams: Promise.resolve({}),
    }),
  );

const JOINED = {
  id: "n1",
  kind: "member_added",
  title: "Ada joined",
  body: "Ada joined the project.",
  metadata: null,
  page: null,
  read_at: null,
  created_at: "2026-11-04T10:00:00Z",
};

beforeEach(() => {
  mockFeed = { items: [JOINED], unread: 1 };
});

describe("ProjectNotificationsPage", () => {
  it("lists the project's notices, each leading to its page, with Mark read while any is unread", async () => {
    await page();
    expect(screen.getByRole("heading", { name: "Notifications" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Ada joined" })).toHaveAttribute("href", "/acme/web/members");
    expect(screen.getByRole("button", { name: "Mark read (1)" })).toBeInTheDocument();
  });

  it("says when there is nothing, and when the feed could not be read, offering no Mark read", async () => {
    mockFeed = { items: [], unread: 0 };
    const { unmount } = await page();
    expect(screen.getByText("Nothing new.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Mark read/ })).toBeNull();
    unmount();
    mockFeed = null;
    await page();
    expect(screen.getByText(/Could not be read/)).toBeInTheDocument();
  });
});
