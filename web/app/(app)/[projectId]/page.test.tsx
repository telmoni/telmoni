// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { NotificationFeed } from "@/lib/server/data";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("@/components/local-time", () => ({
  LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span>,
}));

vi.mock("./_activity", () => ({
  MarkProjectRead: ({ unread }: { unread: number }) => (
    <button data-testid="mark-read">Mark read ({unread})</button>
  ),
}));

let mockProject: { id: string; name: string; role: string } | null;
let mockFeed: NotificationFeed | null;

vi.mock("@/lib/server/data", () => ({
  fetchProject: async () => mockProject,
  fetchProjectNotifications: async () => mockFeed,
}));

import OverviewPage from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function item(over: Partial<NotificationFeed["items"][number]> = {}) {
  return {
    id: "feed_1",
    kind: "member_added",
    title: "Ada joined the project",
    body: "Ada accepted the invitation and is now admin on the project.",
    read_at: null,
    created_at: "2026-09-20T10:00:00Z",
    ...over,
  };
}

async function renderAt(projectId: string) {
  render(await OverviewPage({ params: Promise.resolve({ projectId }) }));
}

beforeEach(() => {
  mockProject = { id: PROJECT, name: "Platform", role: "owner" };
  mockFeed = { items: [], unread: 0 };
});

describe("OverviewPage", () => {
  it("renders the Overview header", async () => {
    await renderAt(PROJECT);
    expect(screen.getByTestId("page-header")).toHaveTextContent("Overview");
  });

  it("names the project, and says nothing else about it", async () => {
    await renderAt(PROJECT);
    expect(screen.getByText("Platform")).toBeInTheDocument();
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
  });

  it("shows the outage card when the project cannot be read", async () => {
    mockProject = null;
    await renderAt(PROJECT);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByText(PROJECT)).toBeNull();
  });
});

// This section is the only reader of a project-scoped notice. The
// organization's own feed is a different set and cannot show these.
describe("OverviewPage project activity", () => {
  it("lists a project notice with its kind", async () => {
    mockFeed = { items: [item()], unread: 1 };
    await renderAt(PROJECT);
    expect(screen.getByText("Ada joined the project")).toBeInTheDocument();
    expect(screen.getByText("Member added")).toBeInTheDocument();
  });

  it("shows the connector kinds the page exists to surface", async () => {
    mockFeed = {
      items: [
        item({
          id: "f1",
          kind: "connector_connected",
          title: "Slack connected",
        }),
        item({
          id: "f2",
          kind: "connector_disconnected",
          title: "Slack disconnected",
        }),
      ],
      unread: 2,
    };
    await renderAt(PROJECT);
    expect(screen.getByText("Channel connected")).toBeInTheDocument();
    expect(screen.getByText("Channel disconnected")).toBeInTheDocument();
  });

  // The count is the service's, not the page's: a backlog past the page cap
  // would be undercounted by counting the rendered rows.
  it("offers mark-read only while something is unread", async () => {
    mockFeed = { items: [item()], unread: 3 };
    await renderAt(PROJECT);
    expect(screen.getByTestId("mark-read")).toHaveTextContent("Mark read (3)");
  });

  it("hides mark-read when everything is read", async () => {
    mockFeed = {
      items: [item({ read_at: "2026-09-20T11:00:00Z" })],
      unread: 0,
    };
    await renderAt(PROJECT);
    expect(screen.queryByTestId("mark-read")).toBeNull();
  });

  it("says what will appear here when the feed is empty", async () => {
    mockFeed = { items: [], unread: 0 };
    await renderAt(PROJECT);
    expect(screen.getByTestId("project-activity-empty")).toBeInTheDocument();
  });

  // Unreachable renders nothing at all: the page's subject is the project,
  // and a feed that cannot be read is not the project's problem.
  it("renders no activity section when the service is unreachable", async () => {
    mockFeed = null;
    await renderAt(PROJECT);
    expect(screen.queryByTestId("project-activity")).toBeNull();
    expect(screen.getByText("Platform")).toBeInTheDocument();
  });
});
