// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
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

let mockProject: { id: string; slug: string; name: string; role: string } | null;
vi.mock("@/lib/server/data", () => ({ fetchProjectBySlug: async () => mockProject }));

let mockAgents: unknown;
vi.mock("@/lib/server/entities/agents", () => ({ fetchAgents: async () => mockAgents }));

let mockAlerts: unknown;
vi.mock("@/lib/server/entities/alerts", () => ({ fetchAlerts: async () => mockAlerts }));

// The panel reads once opened, and is proved by its own suite.
vi.mock("./_versions-panel", () => ({
  VersionsHost: ({ projectId, days, children }: { projectId: string; days: number; children: React.ReactNode }) => (
    <div data-testid="versions-host" data-project={projectId} data-days={days}>
      {children}
    </div>
  ),
  VersionsOpener: ({ agent }: { agent: string }) => <button type="button">{`Versions of ${agent}`}</button>,
}));

import AgentsPage, { generateMetadata } from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });

const agent = (name: string, patch: Record<string, unknown> = {}) => ({
  agent: name,
  traces: 1,
  failed_traces: 0,
  last_trace_id: "0191a3b47c2e7d3f8e4a5b6c7d8e9f01",
  last_started_at: "2026-11-05T09:00:00Z",
  last_ended: true,
  last_status: "ok",
  last_spend: 0.012,
  usual_spend: 0.012,
  ...patch,
});

// An alert over every agent where `agents` is empty, held to them where not.
const alert = (id: string, agents: string[] = [], paused: string | null = null) => ({
  id,
  filters: agents.length > 0 ? { agent: agents } : {},
  paused,
});

const incident = (id: string, rule: string, status = "open") => ({ id, rule_id: rule, status });

const agentsOk = (agents: unknown[], changes: unknown[] = []) => ({
  kind: "ok",
  value: { days: 14, agents, changes },
});

const change = (kind: string, value: string, trace: string, patch: Record<string, unknown> = {}) => ({
  agent: "support-bot",
  environment: "production",
  kind,
  value,
  changed_at: "2026-11-05T08:00:00Z",
  trace_id: trace,
  capped: false,
  ...patch,
});

const alertsOk =(alerts: unknown[], incidents: unknown[] = []) => ({
  kind: "ok",
  alerts,
  incidents,
  mutes: [],
});

describe("a project's Agents page", () => {
  beforeEach(() => {
    mockProject = { id: "prj_1", slug: "web", name: "Web", role: "member" };
    mockAgents = agentsOk([]);
    mockAlerts = alertsOk([]);
  });

  it("titles the tab by the project's name, and the page Agents", async () => {
    expect(await generateMetadata({ params: PARAMS } as never)).toEqual({ title: "Web" });
    render(await AgentsPage({ params: PARAMS } as never));
    expect(screen.getByRole("heading", { name: "Agents" })).toBeInTheDocument();
  });

  it("opens how to connect another agent in a tab of its own", async () => {
    render(await AgentsPage({ params: PARAMS } as never));
    const connect = screen.getByRole("link", { name: /Connect an agent/ });
    expect(connect).toHaveAttribute("href", "/docs/getting-started/connect-an-agent");
    expect(connect).toHaveAttribute("target", "_blank");
  });

  // The first trace made the project's starter alerts, each over every agent.
  it("reads a project with one trace as watched", async () => {
    mockAgents = agentsOk([agent("support-bot")]);
    mockAlerts = alertsOk([alert("failing"), alert("looping"), alert("stalled")]);
    render(await AgentsPage({ params: PARAMS } as never));

    const row = screen.getByRole("link", { name: "support-bot" }).closest("tr")!;
    expect(screen.getByRole("link", { name: "support-bot" })).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot",
    );
    expect(within(row).getByText("OK")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "3 alerts" })).toHaveAttribute("href", "/acme/web/alerts");
    expect(within(row).getByText("$0.012")).toBeInTheDocument();
    expect(within(row).getByText("Usually $0.012")).toBeInTheDocument();
    expect(screen.queryByText(/No agent has run/)).toBeNull();
  });

  it("gives each agent's row a Versions button under the table's one panel, the name still its traces' link", async () => {
    mockAgents = agentsOk([agent("support-bot"), agent("..")]);
    render(await AgentsPage({ params: PARAMS } as never));

    for (const name of ["support-bot", ".."]) {
      const row = screen.getByRole("link", { name }).closest("tr")!;
      const opener = within(row).getByRole("button", { name: `Versions of ${name}` });
      const host = opener.closest("[data-testid='versions-host']");
      expect(host).toHaveAttribute("data-project", "prj_1");
      expect(host).toHaveAttribute("data-days", "14");
    }
  });

  it("links each agent's last trace, and counts its failures and what it cost against its usual", async () => {
    mockAgents = agentsOk([
      agent("nightly-report", { traces: 1, last_ended: false, last_spend: 0.004, usual_spend: null }),
      agent("support-bot", { traces: 12, failed_traces: 2, last_status: "error", last_spend: 0.2, usual_spend: 0.05 }),
    ]);
    render(await AgentsPage({ params: PARAMS } as never));

    const nightly = screen.getByRole("link", { name: "nightly-report" }).closest("tr")!;
    expect(within(nightly).getByText("Running")).toBeInTheDocument();
    expect(within(nightly).getByText("No trace ended yet")).toBeInTheDocument();
    expect(within(nightly).getByRole("link", { name: "No alert yet" })).toHaveAttribute("href", "/acme/web/alerts");

    const support = screen.getByRole("link", { name: "support-bot" }).closest("tr")!;
    expect(within(support).getByText("12")).toBeInTheDocument();
    expect(within(support).getByText("2 failed")).toBeInTheDocument();
    expect(within(support).getByText("$0.20")).toBeInTheDocument();
    expect(within(support).getByText("Usually $0.050")).toBeInTheDocument();
    expect(support.querySelector('time[datetime="2026-11-05T09:00:00Z"]')?.closest("a")).toHaveAttribute(
      "href",
      "/acme/web/traces?agent=support-bot&peek=0191a3b47c2e7d3f8e4a5b6c7d8e9f01",
    );
  });

  it("puts an agent's newest release, model and prompt version under its name, each opening the trace that first carried it", async () => {
    const release = "0192f3a17c2e7b4d9a61000000000001";
    const model = "0192f3a17c2e7b4d9a61000000000002";
    const prompt = "0192f3a17c2e7b4d9a61000000000003";
    // In the server's order, by the kinds' names: the page stands the release first.
    mockAgents = agentsOk(
      [agent("support-bot"), agent("nightly-report")],
      [
        change("model", "gpt-4o-mini", model, { environment: "" }),
        change("prompt", "3", prompt, { prompt: "triage" }),
        change("release", "1.4.2", release),
      ],
    );
    render(await AgentsPage({ params: PARAMS } as never));

    const support = within(screen.getByRole("link", { name: "support-bot" }).closest("td")!);
    expect(support.getAllByRole("link").map((link) => [link.textContent, link.getAttribute("href")])).toEqual([
      ["support-bot", "/acme/web/traces?agent=support-bot"],
      ["release 1.4.2 in production", `/acme/web/traces?agent=support-bot&peek=${release}`],
      ["model gpt-4o-mini", `/acme/web/traces?agent=support-bot&peek=${model}`],
      ["prompt triage version 3 in production", `/acme/web/traces?agent=support-bot&peek=${prompt}`],
    ]);
    const nightly = within(screen.getByRole("link", { name: "nightly-report" }).closest("td")!);
    expect(nightly.getAllByRole("link")).toHaveLength(1);
  });

  // A capped day's last shown change may not be the agent's latest: its line says so.
  it("says an agent's capped change was its day's last of its kind marked", async () => {
    const trace = "0192f3a17c2e7b4d9a61000000000004";
    mockAgents = agentsOk([agent("support-bot")], [change("release", "9.0.24", trace, { capped: true })]);
    render(await AgentsPage({ params: PARAMS } as never));

    expect(screen.getByRole("link", { name: /^release 9\.0\.24 in production/ })).toHaveTextContent(
      "release 9.0.24 in production; its later release changes that UTC day are not marked",
    );
  });

  // An alert held to one agent counts on its row; one over every agent counts
  // once, above them, since no one row is its; a paused one watches nothing.
  it("counts an agent's own open incidents on its row, and every agent's above the table", async () => {
    mockAgents = agentsOk([agent("support-bot"), agent("nightly-report")]);
    mockAlerts = alertsOk(
      [alert("held", ["support-bot"]), alert("every"), alert("paused", ["nightly-report"], "member")],
      [incident("i1", "held"), incident("i2", "every"), incident("i3", "held", "resolved")],
    );
    render(await AgentsPage({ params: PARAMS } as never));

    expect(screen.getByText("1 incident is open on alerts over every agent.")).toBeInTheDocument();
    const support = screen.getByRole("link", { name: "support-bot" }).closest("tr")!;
    expect(within(support).getByRole("link", { name: "2 alerts" })).toBeInTheDocument();
    expect(within(support).getByText("1 open incident")).toBeInTheDocument();
    const nightly = screen.getByRole("link", { name: "nightly-report" }).closest("tr")!;
    expect(within(nightly).getByRole("link", { name: "1 alert" })).toBeInTheDocument();
    expect(within(nightly).queryByText(/open incident/)).toBeNull();
  });

  it("says how to send a first trace where no agent has run", async () => {
    render(await AgentsPage({ params: PARAMS } as never));
    expect(screen.getByText("No agent has run here in the last 14 days.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "API keys" })).toHaveAttribute("href", "/acme/web/api-keys");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("draws the agents with nothing said of their alerts when those could not be read", async () => {
    mockAgents = agentsOk([agent("support-bot")]);
    mockAlerts = { kind: "unavailable" };
    render(await AgentsPage({ params: PARAMS } as never));
    const row = screen.getByRole("link", { name: "support-bot" }).closest("tr")!;
    expect(within(row).getByText("—")).toBeInTheDocument();
  });

  it("says the service is out when the agents could not be read, or the project", async () => {
    mockAgents = { kind: "unavailable" };
    const { unmount } = render(await AgentsPage({ params: PARAMS } as never));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    unmount();
    mockProject = null;
    render(await AgentsPage({ params: PARAMS } as never));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
