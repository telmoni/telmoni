// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { NotificationFeed } from "@/lib/server/data";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("@/components/local-time", () => ({
  LocalTime: ({ iso }: { iso: string }) => <span>{iso}</span>,
}));

vi.mock("./_activity", () => ({
  MarkProjectRead: ({ projectId, unread }: { projectId: string; unread: number }) => (
    <button data-testid="mark-read" data-project={projectId}>
      Mark read ({unread})
    </button>
  ),
}));

let mockProject: { id: string; slug: string; name: string; role: string } | null;
let mockFeed: NotificationFeed | null;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<typeof mockProject>>(
  async () => mockProject,
);
const fetchProjectNotifications = vi.fn<(projectId: string) => Promise<typeof mockFeed>>(
  async () => mockFeed,
);

vi.mock("@/lib/server/data", () => ({
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchProjectNotifications: (projectId: string) => fetchProjectNotifications(projectId),
}));

import OverviewPage from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SLUG = "platform";

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

async function renderAt(project: string) {
  render(await OverviewPage({ params: Promise.resolve({ project }) }));
}

beforeEach(() => {
  vi.clearAllMocks();
  mockProject = { id: PROJECT, slug: SLUG, name: "Platform", role: "owner" };
  mockFeed = { items: [], unread: 0 };
});

describe("OverviewPage", () => {
  it("renders the Overview header", async () => {
    await renderAt(SLUG);
    expect(screen.getByTestId("page-header")).toHaveTextContent("Overview");
  });

  it("names the project, and says nothing else about it", async () => {
    await renderAt(SLUG);
    expect(screen.getByText("Platform")).toBeInTheDocument();
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
  });

  it("shows the outage card when the project cannot be read", async () => {
    mockProject = null;
    await renderAt(SLUG);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(fetchProjectNotifications).not.toHaveBeenCalled();
  });

  // The path names the project by its slug; the services key on its id, and
  // so does everything the page hands on.
  it("finds the project by the path's slug and reads its feed by id", async () => {
    mockFeed = { items: [item()], unread: 1 };
    await renderAt(SLUG);
    expect(fetchProjectBySlug).toHaveBeenCalledWith(SLUG);
    expect(fetchProjectNotifications).toHaveBeenCalledWith(PROJECT);
    expect(screen.getByTestId("mark-read")).toHaveAttribute("data-project", PROJECT);
  });
});

// This section is the only reader of a project-scoped notice. The
// organization's own feed is a different set and cannot show these.
describe("OverviewPage project activity", () => {
  it("lists a project notice with its kind", async () => {
    mockFeed = { items: [item()], unread: 1 };
    await renderAt(SLUG);
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
    await renderAt(SLUG);
    expect(screen.getByText("Channel connected")).toBeInTheDocument();
    expect(screen.getByText("Channel disconnected")).toBeInTheDocument();
  });

  // The count is the service's, not the page's: a backlog past the page cap
  // would be undercounted by counting the rendered rows.
  it("offers mark-read only while something is unread", async () => {
    mockFeed = { items: [item()], unread: 3 };
    await renderAt(SLUG);
    expect(screen.getByTestId("mark-read")).toHaveTextContent("Mark read (3)");
  });

  it("hides mark-read when everything is read", async () => {
    mockFeed = {
      items: [item({ read_at: "2026-09-20T11:00:00Z" })],
      unread: 0,
    };
    await renderAt(SLUG);
    expect(screen.queryByTestId("mark-read")).toBeNull();
  });

  it("renders unread notifications with bold text and read with muted text", async () => {
    mockFeed = {
      items: [
        item({ id: "unread-1", title: "Unread Notice", read_at: null }),
        item({ id: "read-1", title: "Read Notice", read_at: "2026-09-20T11:00:00Z" }),
      ],
      unread: 1,
    };
    await renderAt(SLUG);
    const unreadTitle = screen.getByText("Unread Notice").closest("span");
    const readTitle = screen.getByText("Read Notice").closest("span");
    expect(unreadTitle?.parentElement).toHaveClass("font-bold", "text-foreground");
    expect(readTitle?.parentElement).toHaveClass("font-normal", "text-muted-foreground");
  });

  it("says what will appear here when the feed is empty", async () => {
    mockFeed = { items: [], unread: 0 };
    await renderAt(SLUG);
    expect(screen.getByTestId("project-activity-empty")).toBeInTheDocument();
  });

  // Unreachable renders nothing at all: the page's subject is the project,
  // and a feed that cannot be read is not the project's problem.
  it("renders no activity section when the service is unreachable", async () => {
    mockFeed = null;
    await renderAt(SLUG);
    expect(screen.queryByTestId("project-activity")).toBeNull();
    expect(screen.getByText("Platform")).toBeInTheDocument();
  });
});
