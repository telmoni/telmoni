// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({
    title,
    headingId,
    info,
    action,
  }: {
    title: string;
    headingId?: string;
    info?: string;
    action?: React.ReactNode;
  }) => (
    <div data-testid="page-header" data-heading={headingId} data-info={info}>
      <span>{title}</span>
      {action}
    </div>
  ),
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("@/components/access-denied", () => ({
  AccessDenied: ({ what }: { what: string }) => <div data-testid="denied">{what}</div>,
}));

// The page's interactive leaves, each with what it was handed.
vi.mock("./_alert-panel", () => ({
  NewAlert: ({ connections }: { connections: { id: string; name: string }[] | null }) => (
    <button data-testid="new-alert" data-connections={JSON.stringify(connections)}>
      New alert
    </button>
  ),
}));
vi.mock("./_alert-actions", () => ({
  AlertActions: ({ alert }: { alert: { id: string } }) => (
    <span data-testid="alert-actions" data-alert={alert.id} />
  ),
  HearAgain: ({ connectionId }: { connectionId: string }) => (
    <button data-testid="hear-again" data-connection={connectionId}>
      Hear again
    </button>
  ),
}));
vi.mock("./_mute", () => ({
  MuteButton: ({ ruleId }: { ruleId: string | null }) => (
    <button data-testid="mute" data-rule={String(ruleId)}>
      Mute
    </button>
  ),
  EndMute: ({ muteId }: { muteId: string }) => (
    <button data-testid="end-mute" data-mute={muteId}>
      End now
    </button>
  ),
}));
vi.mock("./_incident-actions", () => ({
  IncidentActions: ({ incidentId, ruleId, status }: { incidentId: string; ruleId: string; status: string }) => (
    <span data-testid="incident-actions" data-incident={incidentId} data-rule={ruleId} data-status={status} />
  ),
}));

import { Role } from "@/lib/types/enums";

const NOW = Date.parse("2026-10-26T12:00:00Z");
const RULE = "7a1c0e52-0000-4000-8000-000000000002";
const OTHER_RULE = "8b2d1f63-0000-4000-8000-000000000006";
const SLACK = "3c4e2a74-0000-4000-8000-000000000004";
const HOOK = "4d5f3b85-0000-4000-8000-000000000007";
const RUN = "9b2d1f63-0000-4000-8000-000000000003";

function incident(id: string, over: Record<string, unknown> = {}) {
  return {
    id,
    rule_id: RULE,
    rule_name: "Slow calls",
    status: "open",
    severity: "alert",
    value: 6200,
    opened_at: "2026-10-26T11:00:00Z",
    acknowledged_at: null,
    acknowledged_by: null,
    resolved_at: null,
    resolved_by: null,
    reopened: 0,
    told_to: [],
    runs: [],
    ...over,
  };
}

const SLOW = {
  id: RULE,
  name: "Slow calls",
  kind: "model_call",
  measure: "duration",
  aggregation: "p95",
  filters: { model: ["claude-sonnet-5-5"] },
  window: "hour",
  operator: "gt",
  alert_threshold: 5000,
  warning_threshold: 3000,
  missing_data: "zero",
  renotify_minutes: 30,
  paused: null,
  paused_at: null,
  value: 6200,
  evaluated_at: "2026-10-26T11:59:00Z",
  severity: "alert",
  severity_since: "2026-10-26T11:00:00Z",
  connectors: [
    { connection_id: SLACK, level: "warning", hours: null, muted: false },
    {
      connection_id: HOOK,
      level: "alert",
      hours: { days: [1, 2, 3, 4, 5], from: "09:00", to: "18:00", utc_offset: 60 },
      muted: true,
    },
  ],
  created_at: "2026-10-25T00:00:00Z",
  updated_at: "2026-10-25T00:00:00Z",
};
const CAP = {
  ...SLOW,
  id: OTHER_RULE,
  name: "Cost cap",
  kind: "run",
  measure: "cost",
  aggregation: "max",
  filters: {},
  alert_threshold: 5,
  warning_threshold: null,
  renotify_minutes: null,
  paused: "plan",
  paused_at: "2026-10-26T10:00:00Z",
  value: null,
  severity: "paused",
  connectors: [],
};

let mockContext: unknown;
let mockProject: unknown;
let mockAlerts: unknown;
let mockConnectors: unknown;
let mockMembers: unknown;
let mockIncident: unknown;
let mockOrganization: unknown;
let mockOrganizationMembers: unknown;
const fetchAlerts = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockAlerts);
const fetchIncident = vi.fn<(projectId: string, id: string) => Promise<unknown>>(
  async () => mockIncident,
);
const fetchOrganizationMembers = vi.fn<() => Promise<unknown>>(async () => mockOrganizationMembers);

vi.mock("@/lib/server/data", () => ({
  INCIDENTS_MAX: 3,
  activeOrganization: () => mockOrganization,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
  fetchAlerts: (projectId: string) => fetchAlerts(projectId),
  fetchConnectors: async () => mockConnectors,
  fetchIncident: (projectId: string, id: string) => fetchIncident(projectId, id),
  fetchMembers: async () => mockMembers,
  fetchOrganizationMembers: () => fetchOrganizationMembers(),
}));

import AlertsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });

beforeEach(() => {
  vi.clearAllMocks();
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member };
  mockAlerts = {
    kind: "ok",
    at: NOW,
    alerts: [CAP, SLOW],
    incidents: [
      incident("i-open", { told_to: [SLACK, "5e6a4c96-0000-4000-8000-000000000008"], runs: [RUN] }),
      incident("i-taken", { status: "acknowledged", acknowledged_by: "user_2", acknowledged_at: "2026-10-26T11:10:00Z", severity: "warning" }),
      incident("i-done", { status: "resolved", resolved_at: "2026-10-26T09:00:00Z", reopened: 2 }),
    ],
    mutes: [
      {
        id: "m-all",
        rule_id: null,
        starts_at: "2026-10-26T11:00:00Z",
        ends_at: "2026-10-26T13:00:00Z",
        created_by: "user_1",
        created_at: "2026-10-26T11:00:00Z",
        ended_by: null,
      },
      {
        id: "m-slow",
        rule_id: RULE,
        starts_at: "2026-10-27T09:00:00Z",
        ends_at: "2026-10-27T10:00:00Z",
        created_by: "user_2",
        created_at: "2026-10-26T11:00:00Z",
        ended_by: null,
      },
      {
        id: "m-over",
        rule_id: RULE,
        starts_at: "2026-10-26T08:00:00Z",
        ends_at: "2026-10-26T09:00:00Z",
        created_by: "user_2",
        created_at: "2026-10-26T08:00:00Z",
        ended_by: "user_2",
      },
    ],
  };
  mockConnectors = {
    enabled: { slack: true, webhook: true },
    connections: [
      {
        id: SLACK,
        provider: "slack",
        external_workspace_id: "T1",
        external_workspace_name: "Acme",
        channel_id: "C1",
        channel_name: "alerts",
        status: "active",
        last_error: null,
        last_delivery_at: null,
        created_at: "2026-10-01T00:00:00Z",
        event_kinds: null,
        previous_secret_expires_at: null,
      },
      {
        id: HOOK,
        provider: "webhook",
        external_workspace_id: "webhook",
        external_workspace_name: null,
        channel_id: "abcd1234",
        channel_name: "hooks.example.com",
        status: "errored",
        last_error: "410",
        last_delivery_at: null,
        created_at: "2026-10-01T00:00:00Z",
        event_kinds: null,
        previous_secret_expires_at: null,
      },
    ],
  };
  mockMembers = {
    kind: "ok",
    ceiling: null,
    members: [
      { id: "m1", member_id: "user_1", email: "ada@example.test", display_name: "Ada", role: "member", created_at: "", is_owner: false },
      { id: "m2", member_id: "user_2", email: "sam@example.test", display_name: null, role: "member", created_at: "", is_owner: false },
    ],
  };
  mockIncident = { kind: "missing" };
  // A member of the project, who may not read the organization's roster.
  mockOrganization = { organizationId: "org_1", slug: "acme", name: "Acme", role: "member" };
  mockOrganizationMembers = { kind: "forbidden" };
});

/** An alert's incidents, by their order in its card. */
function incidentItems(ruleId: string) {
  const card = document.getElementById(`alert-${ruleId}`)!;
  return Array.from(card.querySelectorAll<HTMLElement>('li[id^="incident-"]'));
}

/** What an alert's card says beside `label`. */
function detail(ruleId: string, label: string) {
  const card = document.getElementById(`alert-${ruleId}`)!;
  return within(card).getByText(label, { selector: "dt" }).nextElementSibling as HTMLElement;
}

describe("AlertsPage", () => {
  it("offers a new alert and a mute over all", async () => {
    render(await AlertsPage({ params: PARAMS }));
    expect(fetchAlerts).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp");
    const header = screen.getByTestId("page-header");
    // The title takes the focus a control hands it as its alert goes.
    expect(header.dataset.heading).toBe("alerts-heading");
    expect(header.dataset.info).toMatch(/^Each reads a measure of the project's spans/);
    expect(within(header).getByTestId("new-alert").dataset.connections).toBe(
      JSON.stringify([
        { id: SLACK, name: "#alerts in Acme", status: "active" },
        { id: HOOK, name: "hooks.example.com", status: "errored" },
      ]),
    );
    expect(within(header).getByTestId("mute").dataset.rule).toBe("null");
  });

  it("draws every alert with its metric, filters, window, thresholds, severity, connectors and windows", async () => {
    render(await AlertsPage({ params: PARAMS }));
    const slow = document.getElementById(`alert-${RULE}`)!;
    expect(within(slow).getByRole("heading", { name: "Slow calls" })).toBeInTheDocument();
    expect(within(slow).getByText("The p95 duration of model calls over the last hour")).toBeInTheDocument();
    expect(within(slow).getByText("Alert > 5,000 ms, warning > 3,000 ms")).toBeInTheDocument();
    expect(within(slow).getByText("model claude-sonnet-5-5")).toBeInTheDocument();
    expect(within(slow).getByText("Read as 0")).toBeInTheDocument();
    expect(within(slow).getByText("Every 30 minutes")).toBeInTheDocument();
    const connectors = detail(RULE, "Connectors");
    expect(within(connectors).getByText(/#alerts in Acme/)).toHaveTextContent(
      "#alerts in Acme · warnings and alerts",
    );
    expect(within(connectors).getByText(/hooks\.example\.com/)).toHaveTextContent(
      "hooks.example.com · alerts only, Mon–Fri 09:00–18:00 UTC+01:00",
    );
    expect(within(slow).getByTestId("hear-again").dataset.connection).toBe(HOOK);
    // Its own window to come and the one over every alert, the one over not.
    expect(within(slow).getAllByTestId("end-mute").map((b) => b.dataset.mute)).toEqual(["m-all", "m-slow"]);
    expect(within(slow).getByTestId("alert-actions").dataset.alert).toBe(RULE);

    const cap = document.getElementById(`alert-${OTHER_RULE}`)!;
    expect(within(cap).getByText("The most a run spent over the last hour")).toBeInTheDocument();
    expect(within(cap).getByText("Paused")).toBeInTheDocument();
    expect(within(cap).getByText(/Paused by the plan/)).toBeInTheDocument();
    expect(within(cap).getByText("None: its incidents reach the project's feed alone")).toBeInTheDocument();
    expect(within(cap).getByText("Alert > $5.00")).toBeInTheDocument();
    expect(detail(OTHER_RULE, "Incidents")).toHaveTextContent(/^None$/);
  });

  // ⚠ Under the alert that opened them, open ones first, each with where its notices went.
  it("lists each alert's incidents under it as the server ordered them", async () => {
    render(await AlertsPage({ params: PARAMS }));
    const [open, taken, done] = incidentItems(RULE);
    expect(incidentItems(RULE).map((item) => item.id)).toEqual([
      "incident-i-open",
      "incident-i-taken",
      "incident-i-done",
    ]);
    // Its value is the alert's to show, in the unit it measures now.
    expect(open).not.toHaveTextContent("6,200");
    expect(within(open).getByText("Alert")).toBeInTheDocument();
    expect(within(open).getByText("Open")).toBeInTheDocument();
    expect(open).toHaveTextContent("told the feed, #alerts in Acme, a removed connector");
    expect(within(open).getByRole("link", { name: /run 9b2d1f63/ })).toHaveAttribute(
      "href",
      `/acme/web/traces/${RUN}`,
    );
    const actions = within(open).getByTestId("incident-actions");
    expect(actions.dataset.status).toBe("open");
    expect(actions.dataset.rule).toBe(RULE);

    expect(within(taken).getByText("Warning")).toBeInTheDocument();
    expect(within(taken).getByText("Acknowledged by sam@example.test")).toBeInTheDocument();
    expect(taken).toHaveTextContent("told the feed");

    expect(within(done).getByText("Recovered")).toBeInTheDocument();
    expect(done).toHaveTextContent("reopened 2 times");
  });

  // ⚠ A trace links the incident it carries even past a full list.
  it("reads a followed incident past a full list on its own, and marks it under its alert", async () => {
    const past = "5d4f5c68-0000-4000-8000-000000000001";
    mockIncident = { kind: "ok", incident: incident(past, { status: "resolved", resolved_at: "2026-10-01T00:00:00Z" }) };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({ incident: past }) }));
    expect(fetchIncident).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", past);
    const items = incidentItems(RULE);
    expect(items).toHaveLength(4);
    expect(items[3]).toHaveAttribute("id", `incident-${past}`);
    expect(items[3]).toHaveAttribute("aria-current", "true");
  });

  it("marks a followed incident the list holds without reading it again, and ignores one that is no id", async () => {
    const held = "6e5a6d79-0000-4000-8000-00000000000a";
    const alerts = mockAlerts as { incidents: ReturnType<typeof incident>[] };
    alerts.incidents = [incident(held), ...alerts.incidents.slice(1)];
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({ incident: held.toUpperCase() }) }));
    expect(fetchIncident).not.toHaveBeenCalled();
    const [first, ...rest] = incidentItems(RULE);
    expect(first).toHaveAttribute("aria-current", "true");
    expect(first).toHaveAttribute("tabindex", "-1");
    for (const item of rest) expect(item).not.toHaveAttribute("aria-current");

    for (const asked of ["i-open", [held]]) {
      cleanup();
      render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({ incident: asked }) }));
      expect(fetchIncident).not.toHaveBeenCalled();
      for (const item of incidentItems(RULE)) expect(item).not.toHaveAttribute("aria-current");
    }
  });

  it("names the window over every alert at the page's head, with who set it", async () => {
    render(await AlertsPage({ params: PARAMS }));
    const row = screen.getByText("Every alert is muted").closest("div")!.parentElement!;
    expect(row).toHaveTextContent("set by Ada");
    expect(within(row).getByTestId("end-mute").dataset.mute).toBe("m-all");
  });

  it("says when there are no alerts yet", async () => {
    mockAlerts = { ...(mockAlerts as object), alerts: [], mutes: [] };
    render(await AlertsPage({ params: PARAMS }));
    expect(screen.getByText("No alerts yet.")).toBeInTheDocument();
    // Nothing to mute: only New alert stands at the page's head.
    expect(screen.queryByTestId("mute")).toBeNull();
    expect(screen.getByTestId("new-alert")).toBeInTheDocument();
  });

  it("names nobody when the roster cannot be read", async () => {
    mockMembers = { kind: "unavailable" };
    render(await AlertsPage({ params: PARAMS }));
    const row = screen.getByText("Every alert is muted").closest("div")!.parentElement!;
    expect(row).toHaveTextContent("set by someone");
    expect(within(incidentItems(RULE)[1]).getByText("Acknowledged by someone")).toBeInTheDocument();
    expect(fetchOrganizationMembers).not.toHaveBeenCalled();
  });

  // ⚠ An organization's admin acts on every project with no seat on it.
  it("names an admin the project's roster lacks from the organization's, to an admin", async () => {
    const taken = (id: string, by: string) =>
      incident(id, { status: "acknowledged", acknowledged_by: by, acknowledged_at: "2026-10-26T11:10:00Z" });
    mockAlerts = { ...(mockAlerts as object), incidents: [taken("i-admin", "user_admin"), taken("i-gone", "user_gone")] };
    render(await AlertsPage({ params: PARAMS }));
    expect(fetchOrganizationMembers).not.toHaveBeenCalled();
    expect(within(incidentItems(RULE)[0]).getByText("Acknowledged by an organization admin or a former member")).toBeInTheDocument();

    cleanup();
    mockOrganization = { organizationId: "org_1", slug: "acme", name: "Acme", role: "admin" };
    mockOrganizationMembers = {
      kind: "ok",
      ceiling: null,
      members: [{ id: "o1", member_id: "user_admin", email: "grace@example.test", display_name: "Grace", role: "admin", created_at: "" }],
    };
    render(await AlertsPage({ params: PARAMS }));
    const [admin, gone] = incidentItems(RULE);
    expect(within(admin).getByText("Acknowledged by Grace")).toBeInTheDocument();
    expect(within(gone).getByText("Acknowledged by a former member")).toBeInTheDocument();
  });

  it("calls a connection by no wrong name when the connectors cannot be read", async () => {
    mockConnectors = null;
    render(await AlertsPage({ params: PARAMS }));
    const slow = document.getElementById(`alert-${RULE}`)!;
    expect(within(slow).getAllByText(/^A connector/).length).toBe(2);
    expect(within(slow).queryByText(/removed/)).toBeNull();
    expect(incidentItems(RULE)[0]).toHaveTextContent("told the feed, a connector, a connector");
  });

  it("draws the outage and the refusal as every page does", async () => {
    mockAlerts = { kind: "unavailable" };
    render(await AlertsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    cleanup();
    mockAlerts = { kind: "forbidden" };
    render(await AlertsPage({ params: PARAMS }));
    expect(screen.getByTestId("denied")).toHaveTextContent("this project's alerts");
  });
});
