// @vitest-environment jsdom
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      <h1>{title}</h1>
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

// The action, not its effects: which project the page hands it is the
// assertion, and the service answers whether the person is on it.
const markProjectReadAction = vi.hoisted(() =>
  vi.fn<(projectId: string) => Promise<{ error: string | null }>>(async () => ({ error: null })),
);
vi.mock("./notice-actions", () => ({ markProjectReadAction }));

type FeedItem = {
  id: string;
  kind: string;
  title: string;
  body: string;
  read_at: string | null;
  created_at: string;
};
type Feed = { items: FeedItem[]; unread: number } | null;

let mockProject: { id: string; slug: string; name: string; role: string } | null;
let mockFeed: Feed;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<typeof mockProject>>(
  async () => mockProject,
);
const fetchProjectNotifications = vi.fn<(projectId: string) => Promise<Feed>>(
  async () => mockFeed,
);

vi.mock("@/lib/server/data", () => ({
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchProjectNotifications: (projectId: string) => fetchProjectNotifications(projectId),
}));

import OverviewPage, { generateMetadata } from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SLUG = "platform";

const CONNECTED: FeedItem = {
  id: "n2",
  kind: "connector_connected",
  title: "Slack connected",
  body: "This project's notices will now be posted to #agents in Acme.",
  read_at: null,
  created_at: "2026-10-14T03:00:00Z",
};
const JOINED: FeedItem = {
  id: "n1",
  kind: "member_added",
  title: "Grace joined the project",
  body: "Grace accepted an invitation as a Member.",
  read_at: "2026-10-13T09:00:00Z",
  created_at: "2026-10-13T08:00:00Z",
};

async function renderAt(project: string) {
  render(await OverviewPage({ params: Promise.resolve({ organization: "acme", project }) }));
}

beforeEach(() => {
  vi.clearAllMocks();
  mockProject = { id: PROJECT, slug: SLUG, name: "Platform", role: "member" };
  mockFeed = { items: [], unread: 0 };
});

describe("OverviewPage", () => {
  // With nothing raised on it, the page is the project's name alone.
  it("is titled by the project, and says nothing else while its feed is empty", async () => {
    await renderAt(SLUG);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Platform");
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(screen.queryByRole("region", { name: "Project notices" })).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("finds the project by the path's slug, and reads that project's own feed", async () => {
    await renderAt(SLUG);
    expect(fetchProjectBySlug).toHaveBeenCalledWith(SLUG);
    expect(fetchProjectNotifications).toHaveBeenCalledWith(PROJECT);
  });

  // The one place in the console a project's notices are read. Mark read is
  // the feed's own, in its heading row.
  it("shows the project's notices, the unread ones marked, with Mark read", async () => {
    mockFeed = { items: [CONNECTED, JOINED], unread: 1 };
    await renderAt(SLUG);
    const notices = screen.getByRole("region", { name: "Project notices" });
    expect(notices).toHaveTextContent("Slack connected");
    expect(notices).toHaveTextContent("Channel connected");
    expect(notices).toHaveTextContent("Grace joined the project");
    expect(notices).toHaveTextContent("Member added");
    expect(screen.getAllByText("Unread")).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2, name: "Project notices" })).toBeInTheDocument();
    expect(within(notices).getByRole("button", { name: "Mark read (1)" })).toBeInTheDocument();
    expect(within(screen.getByTestId("page-header")).queryAllByRole("button")).toHaveLength(0);
  });

  it("hands Mark read the project's id, not its slug", async () => {
    mockFeed = { items: [CONNECTED], unread: 1 };
    await renderAt(SLUG);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Mark read (1)" }));
    });
    expect(markProjectReadAction).toHaveBeenCalledExactlyOnceWith(PROJECT, CONNECTED.created_at);
  });

  it("offers no Mark read once everything on the feed is read", async () => {
    mockFeed = { items: [JOINED], unread: 0 };
    await renderAt(SLUG);
    expect(screen.getByRole("region", { name: "Project notices" })).toBeInTheDocument();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  // A feed that cannot be read is the notifications service unset or down:
  // the page's subject is the project, so it shows the name and no error.
  it("shows the name alone when the feed cannot be read", async () => {
    mockFeed = null;
    await renderAt(SLUG);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Platform");
    expect(screen.queryByRole("region", { name: "Project notices" })).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("shows the outage card when the project cannot be read, and asks for no feed", async () => {
    mockProject = null;
    await renderAt(SLUG);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByText("Platform")).toBeNull();
    expect(fetchProjectNotifications).not.toHaveBeenCalled();
  });

  it("titles the tab by the project", async () => {
    expect(await generateMetadata({ params: Promise.resolve({ project: SLUG }) })).toEqual({
      title: "Platform",
    });
  });
});
