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
type Monitor = {
  id: string;
  slug: string;
  name: string;
  agent: string;
  schedule: string | null;
  interval_s: number | null;
  timezone: string;
  grace_s: number | null;
  expected_duration_s: number | null;
  bounds: { stall_after_s: number | null; max_steps: number | null; max_cost_usd: number | null };
  state: string;
  state_changed_at: string;
  next_due_at: string | null;
  last_run: {
    run_id: string;
    started_at: string;
    ended_at: string;
    status: string;
    steps: number | null;
    cost_usd: number | null;
  } | null;
  last_success_at: string | null;
  created_at: string;
  updated_at: string;
};
type Monitors =
  | { kind: "ok"; monitors: Monitor[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

let mockProject: { id: string; slug: string; name: string; role: string } | null;
let mockFeed: Feed;
let mockMonitors: Monitors;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<typeof mockProject>>(
  async () => mockProject,
);
const fetchProjectNotifications = vi.fn<(projectId: string) => Promise<Feed>>(
  async () => mockFeed,
);
const fetchMonitors = vi.fn<(projectId: string) => Promise<Monitors>>(async () => mockMonitors);

vi.mock("@/lib/server/data", () => ({
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchProjectNotifications: (projectId: string) => fetchProjectNotifications(projectId),
  fetchMonitors: (projectId: string) => fetchMonitors(projectId),
}));

import OverviewPage, { generateMetadata } from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SLUG = "platform";

const DISCOVERED: FeedItem = {
  id: "n2",
  kind: "monitor_discovered",
  title: "New monitor: nightly-report",
  body: "A ping under the key telmoniping_ab… named a monitor this project did not have.",
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

/** A monitor in `state`, named for it, its last run ended well. */
function monitor(state: string, name: string, last_run: Monitor["last_run"] = ENDED): Monitor {
  return {
    id: `m-${name}`,
    slug: name,
    name,
    agent: name,
    schedule: null,
    interval_s: 3600,
    timezone: "UTC",
    grace_s: 60,
    expected_duration_s: null,
    bounds: { stall_after_s: 120, max_steps: 40, max_cost_usd: null },
    state,
    state_changed_at: "2026-10-21T03:12:00Z",
    next_due_at: "2026-10-21T04:00:00Z",
    last_run,
    last_success_at: null,
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-21T03:12:00Z",
  };
}
const ENDED: Monitor["last_run"] = {
  run_id: "5d4f5c68-0000-4000-8000-000000000001",
  started_at: "2026-10-21T03:00:00Z",
  ended_at: "2026-10-21T03:04:02Z",
  status: "ok",
  steps: 12,
  cost_usd: 0.08,
};

async function renderAt(project: string) {
  render(await OverviewPage({ params: Promise.resolve({ organization: "acme", project }) }));
}

function board() {
  return screen.getByRole("region", { name: "Monitors" });
}

beforeEach(() => {
  vi.clearAllMocks();
  mockProject = { id: PROJECT, slug: SLUG, name: "Platform", role: "member" };
  mockFeed = { items: [], unread: 0 };
  mockMonitors = { kind: "ok", monitors: [] };
});

describe("OverviewPage", () => {
  // With no monitor yet, the board says what one is; with nothing raised
  // on the project, there is no feed.
  it("is titled by the project, and opens on an empty board while it has no monitor", async () => {
    await renderAt(SLUG);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Platform");
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(board()).toHaveTextContent("No monitors yet.");
    expect(board()).toHaveTextContent("The first ping to a slug makes one.");
    expect(screen.queryByRole("region", { name: "Project notices" })).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("finds the project by the path's slug, and reads that project's monitors and feed", async () => {
    await renderAt(SLUG);
    expect(fetchProjectBySlug).toHaveBeenCalledWith(SLUG);
    expect(fetchMonitors).toHaveBeenCalledWith(PROJECT);
    expect(fetchProjectNotifications).toHaveBeenCalledWith(PROJECT);
  });

  // The board: the monitors that need someone, the worst first, then those
  // running, each with its last run and a link to its page; every other
  // counted in one line.
  it("lists the monitors that need someone, then those running, and counts the rest", async () => {
    mockMonitors = {
      kind: "ok",
      monitors: [
        monitor("failed", "nightly-report"),
        monitor("late", "hourly-sync", null),
        monitor("running", "support-bot"),
        monitor("new", "found-by-ping"),
        monitor("healthy", "weekly-digest"),
        monitor("healthy", "daily-brief"),
      ],
    };
    await renderAt(SLUG);
    const rows = within(board()).getAllByText(/^(nightly-report|hourly-sync|support-bot)$/);
    expect(rows.map((r) => r.textContent)).toEqual([
      "nightly-report",
      "hourly-sync",
      "support-bot",
    ]);
    expect(board()).toHaveTextContent("Failed");
    expect(board()).toHaveTextContent("Late");
    expect(board()).toHaveTextContent("Running");
    expect(board()).toHaveTextContent("Last run");
    expect(board()).toHaveTextContent("OK · 4 min 2 s · 12 / 40 steps · $0.08");
    expect(board()).toHaveTextContent("No run has ended yet");
    expect(board()).toHaveTextContent("1 new · 2 healthy");
    expect(within(board()).queryByText("weekly-digest")).toBeNull();
    expect(within(board()).queryByText("found-by-ping")).toBeNull();
    expect(
      within(board())
        .getAllByRole("link")
        .map((link) => link.getAttribute("href")),
    ).toEqual([
      `/acme/${SLUG}/monitors/nightly-report`,
      `/acme/${SLUG}/monitors/hourly-sync`,
      `/acme/${SLUG}/monitors/support-bot`,
    ]);
  });

  it("says nothing needs anyone when every monitor is healthy", async () => {
    mockMonitors = { kind: "ok", monitors: [monitor("healthy", "a"), monitor("paused", "b")] };
    await renderAt(SLUG);
    expect(board()).toHaveTextContent("1 paused · 1 healthy");
    expect(board()).toHaveTextContent("Nothing needs anyone, and no run is going.");
    expect(within(board()).queryByText("a")).toBeNull();
  });

  // Telemetry down is said, never hidden: a board that went quiet would read
  // as every monitor well.
  it("says when the monitors cannot be read, and still shows the feed", async () => {
    mockMonitors = { kind: "unavailable" };
    mockFeed = { items: [JOINED], unread: 0 };
    await renderAt(SLUG);
    expect(board()).toHaveTextContent("Monitors are unavailable right now");
    expect(board()).not.toHaveTextContent("healthy");
    expect(screen.getByRole("region", { name: "Project notices" })).toBeInTheDocument();
  });

  it("says when the monitors may not be read", async () => {
    mockMonitors = { kind: "forbidden" };
    await renderAt(SLUG);
    expect(board()).toHaveTextContent("You can't read this project's monitors.");
  });

  // Every monitor on the board leaves nothing to count, and the line goes.
  it("counts nothing when every monitor is on the board", async () => {
    mockMonitors = { kind: "ok", monitors: [monitor("failed", "a"), monitor("running", "b")] };
    await renderAt(SLUG);
    expect(within(board()).getByText("a")).toBeInTheDocument();
    expect(within(board()).getByText("b")).toBeInTheDocument();
    expect(board()).not.toHaveTextContent("none");
    expect(board()).not.toHaveTextContent("healthy");
  });

  // The one place in the console a project's notices are read: a monitor a
  // ping made among them, which tells its owner nowhere else. Mark read is
  // the feed's own, in its heading row.
  it("shows the project's notices under the board, the unread ones marked, with Mark read", async () => {
    mockFeed = { items: [DISCOVERED, JOINED], unread: 1 };
    await renderAt(SLUG);
    const notices = screen.getByRole("region", { name: "Project notices" });
    expect(notices).toHaveTextContent("New monitor: nightly-report");
    expect(notices).toHaveTextContent("Monitor discovered");
    expect(notices).toHaveTextContent("Grace joined the project");
    expect(notices).toHaveTextContent("Member added");
    expect(screen.getAllByText("Unread")).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 2, name: "Project notices" })).toBeInTheDocument();
    expect(within(notices).getByRole("button", { name: "Mark read (1)" })).toBeInTheDocument();
    expect(within(screen.getByTestId("page-header")).queryAllByRole("button")).toHaveLength(0);
    expect(board().compareDocumentPosition(notices) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("hands Mark read the project's id, not its slug", async () => {
    mockFeed = { items: [DISCOVERED], unread: 1 };
    await renderAt(SLUG);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Mark read (1)" }));
    });
    expect(markProjectReadAction).toHaveBeenCalledExactlyOnceWith(PROJECT, DISCOVERED.created_at);
  });

  it("offers no Mark read once everything on the feed is read", async () => {
    mockFeed = { items: [JOINED], unread: 0 };
    await renderAt(SLUG);
    expect(screen.getByRole("region", { name: "Project notices" })).toBeInTheDocument();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  // A feed that cannot be read is the notifications service unset or down:
  // the page's subject is the project, so it shows the board and no error.
  it("shows the board alone when the feed cannot be read", async () => {
    mockFeed = null;
    await renderAt(SLUG);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Platform");
    expect(board()).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Project notices" })).toBeNull();
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("shows the outage card when the project cannot be read, and asks for nothing else", async () => {
    mockProject = null;
    await renderAt(SLUG);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByText("Platform")).toBeNull();
    expect(fetchMonitors).not.toHaveBeenCalled();
    expect(fetchProjectNotifications).not.toHaveBeenCalled();
  });

  it("titles the tab by the project", async () => {
    expect(await generateMetadata({ params: Promise.resolve({ project: SLUG }) })).toEqual({
      title: "Platform",
    });
  });
});
