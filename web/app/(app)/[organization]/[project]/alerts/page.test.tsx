// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";

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
vi.mock("../connectors/_webhook", () => ({
  ConnectWebhookButton: ({ label }: { label: React.ReactNode }) => <button type="button">{label}</button>,
}));
vi.mock("./_incident-actions", () => ({
  IncidentActions: ({ incidentId, ruleId, status }: { incidentId: string; ruleId: string; status: string }) => (
    <span data-testid="incident-actions" data-incident={incidentId} data-rule={ruleId} data-status={status} />
  ),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));

import { ChangeSchema } from "@/lib/server/entities/changes";
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
    changes: [],
    changes_more: false,
    ...over,
  };
}

/** A change as the lane answers it, read through the entity's own schema. */
function change(over: Record<string, unknown> = {}) {
  return ChangeSchema.parse({
    agent: "support",
    environment: "production",
    kind: "release",
    value: "2026.11.3",
    changed_at: "2026-10-26T10:46:00Z",
    run_id: RUN,
    capped: false,
    ...over,
  });
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
  score_name: null,
  baseline: "none",
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
  starter: false,
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
  score_name: null,
  baseline: "none",
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
  fetchHistory: async () => null,
  fetchIncident: (projectId: string, id: string) => fetchIncident(projectId, id),
  fetchMembers: async () => mockMembers,
  fetchOrganizationMembers: () => fetchOrganizationMembers(),
}));

import AlertsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });

beforeEach(() => {
  vi.clearAllMocks();
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test" },
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
    startersWithheld: [],
    startersDetail: null,
    incidents: [
      incident("i-open", { told_to: [SLACK, "5e6a4c96-0000-4000-8000-000000000008"], runs: [RUN] }),
      incident("i-taken", { status: "acknowledged", acknowledged_by: "user_2", acknowledged_at: "2026-10-26T11:10:00Z", severity: "warning" }),
      incident("i-done", { status: "resolved", resolved_at: "2026-10-26T09:00:00Z", reopened: 2 }),
    ],
    mutes: [
      {
        id: "m-all",
        rule_id: null,
        monitor_id: null,
        starts_at: "2026-10-26T11:00:00Z",
        ends_at: "2026-10-26T13:00:00Z",
        created_by: "user_1",
        created_at: "2026-10-26T11:00:00Z",
        ended_by: null,
      },
      {
        id: "m-slow",
        rule_id: RULE,
        monitor_id: null,
        starts_at: "2026-10-27T09:00:00Z",
        ends_at: "2026-10-27T10:00:00Z",
        created_by: "user_2",
        created_at: "2026-10-26T11:00:00Z",
        ended_by: null,
      },
      {
        id: "m-over",
        rule_id: RULE,
        monitor_id: null,
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
      { id: "m1", memberId: "user_1", email: "ada@example.test", displayName: "Ada", role: "member", createdAt: "" },
      { id: "m2", memberId: "user_2", email: "sam@example.test", displayName: null, role: "member", createdAt: "" },
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

/** The nth incident under an alert, which the test fails outright without. */
function incidentItem(ruleId: string, n: number): HTMLElement {
  const item = incidentItems(ruleId)[n];
  if (!item) throw new Error(`no incident ${n} under ${ruleId}`);
  return item;
}

/** What an alert's card says beside `label`. */
function detail(ruleId: string, label: string) {
  const card = document.getElementById(`alert-${ruleId}`)!;
  return within(card).getByText(label, { selector: "dt" }).nextElementSibling as HTMLElement;
}

const CSS = readFileSync(path.join(__dirname, "../../../../globals.css"), "utf8");

type Rgb = [number, number, number];

/** A colour token of a console palette, as `globals.css` sets it in oklch. */
function oklch(theme: string, name: string): Rgb | null {
  const at = CSS.indexOf(`\n${theme} {`);
  const block = CSS.slice(at, CSS.indexOf("\n}", at));
  const value = new RegExp(`--${name}:\\s*oklch\\(\\s*([\\d.]+)\\s+([\\d.]+)\\s+([\\d.]+)\\s*\\)`).exec(block);
  return value ? [Number(value[1]), Number(value[2]), Number(value[3])] : null;
}

/** An oklch colour in gamma-encoded sRGB, where a browser lays one over another. */
function srgb([l, c, h]: Rgb): Rgb {
  const a = c * Math.cos((h * Math.PI) / 180);
  const b = c * Math.sin((h * Math.PI) / 180);
  const long = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const medium = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const short = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const gamma = (v: number) => {
    const x = Math.min(Math.max(v, 0), 1);
    return x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
  };
  return [
    gamma(4.0767416621 * long - 3.3077115913 * medium + 0.2309699292 * short),
    gamma(-1.2684380046 * long + 2.6097574011 * medium - 0.3413193965 * short),
    gamma(-0.0041960863 * long - 0.7034186147 * medium + 1.707614701 * short),
  ];
}

/** `one` laid over `other` at `alpha`, as a browser blends them. */
function over([r, g, b]: Rgb, [pr, pg, pb]: Rgb, alpha: number): Rgb {
  const mix = (top: number, under: number) => alpha * top + (1 - alpha) * under;
  return [mix(r, pr), mix(g, pg), mix(b, pb)];
}

/** WCAG's contrast between two sRGB colours. */
function contrast(one: Rgb, other: Rgb): number {
  const linear = (x: number) => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
  const luminance = ([r, g, b]: Rgb) => 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  const high = Math.max(luminance(one), luminance(other));
  const low = Math.min(luminance(one), luminance(other));
  return (high + 0.05) / (low + 0.05);
}

describe("AlertsPage", () => {
  it("offers a new alert and a mute over all", async () => {
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchAlerts).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp");
    const header = screen.getByTestId("page-header");
    // The title takes the focus a control hands it as its alert goes.
    expect(header.dataset.heading).toBe("alerts-heading");
    expect(header.dataset.info).toMatch(/^Each reads a measure of the project's spans/);
    expect(within(header).getByTestId("new-alert").dataset.connections).toBe(
      JSON.stringify([
        { id: SLACK, name: "#alerts in Acme", status: "active", eventKinds: null },
        { id: HOOK, name: "hooks.example.com", status: "errored", eventKinds: null },
      ]),
    );
    expect(within(header).getByTestId("mute").dataset.rule).toBe("null");
  });

  it("draws every alert with its metric, filters, window, thresholds, severity, connectors and windows", async () => {
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
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
    expect(detail(OTHER_RULE, "Now")).toHaveTextContent(
      /^Paused by the plan since \S.*: the organization has more alerts than its plan holds/,
    );
    expect(detail(OTHER_RULE, "Now").querySelector("time")).toHaveAttribute("datetime", "2026-10-26T10:00:00Z");
    expect(within(cap).getByText("None: its incidents reach the project's feed alone")).toBeInTheDocument();
    expect(within(cap).getByText("Alert > $5.00")).toBeInTheDocument();
    expect(detail(OTHER_RULE, "Incidents")).toHaveTextContent(/^None$/);
  });

  // ⚠ Notifications sends a webhook only the kinds it takes: linked, one that
  // takes neither of an incident's, or one alone, hears less than the card says.
  it("marks a linked webhook that takes neither of an incident's kinds, or one alone", async () => {
    const taking = async (kinds: string[] | null) => {
      cleanup();
      const listing = mockConnectors as { connections: { id: string }[] };
      mockConnectors = {
        ...listing,
        connections: listing.connections.map((c) => (c.id === HOOK ? { ...c, event_kinds: kinds } : c)),
      };
      render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      return within(detail(RULE, "Connectors")).getByText(/hooks\.example\.com/).closest("li")!;
    };
    expect(await taking(["member_added"])).toHaveTextContent("Takes no incidents");
    expect(await taking(["incident_opened", "member_left"])).toHaveTextContent("Takes openings only");
    expect(await taking(["incident_resolved"])).toHaveTextContent("Takes resolutions only");
    for (const kinds of [null, ["incident_resolved", "incident_opened"]]) {
      expect(await taking(kinds)).not.toHaveTextContent(/Takes/);
    }
    // Slack takes every kind.
    expect(within(detail(RULE, "Connectors")).getByText(/#alerts in Acme/).closest("li")).not.toHaveTextContent(
      /Takes/,
    );
  });

  // ⚠ One level under the page's h1 (`PageHeader`), so the outline skips none;
  // its id and tabIndex take the focus a control on its card hands it as it goes.
  it("heads each alert one level under the page's title", async () => {
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const slow = document.getElementById(`alert-${RULE}`)!;
    const heading = within(slow).getByRole("heading", { level: 2, name: "Slow calls" });
    expect(heading).toHaveAttribute("id", `alert-${RULE}-name`);
    expect(heading).toHaveAttribute("tabindex", "-1");
    expect(heading).toHaveClass("min-w-0", "truncate", "text-base", "font-medium");
  });

  // ⚠ Kept with no span yet, an alert has no severity to keep, and says so once read.
  it("says where an alert stands before its first reading, after it, and after an empty one it keeps", async () => {
    const standing = async (over: Record<string, unknown>) => {
      cleanup();
      mockAlerts = { ...(mockAlerts as object), alerts: [{ ...SLOW, ...over }] };
      render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      const card = document.getElementById(`alert-${RULE}`)!;
      return { badge: card.querySelector('[data-slot="severity"]'), now: detail(RULE, "Now") };
    };
    const kept = {
      missing_data: "keep",
      severity: "unknown",
      value: null,
      evaluated_at: "2026-10-26T11:59:00Z",
      severity_since: "2026-10-26T11:00:00Z",
    };
    let shown = await standing(kept);
    expect(shown.now).toHaveTextContent(
      /^Its window has held nothing to read yet, so it has no severity to keep; read \S.*\.$/,
    );
    expect(shown.now.querySelector("time")).toHaveAttribute("datetime", "2026-10-26T11:59:00Z");
    expect(shown.badge).toHaveTextContent(/^No severity$/);

    // Made or resumed, and not read since.
    shown = await standing({ ...kept, evaluated_at: null });
    expect(shown.badge).toHaveTextContent(/^Not evaluated$/);
    expect(shown.now).toHaveTextContent(/^Not evaluated yet: it is, within a minute\.$/);
    // Run again by the plan, which keeps the reading from before its pause.
    shown = await standing({ ...kept, value: 6200, evaluated_at: "2026-10-26T09:00:00Z", severity_since: "2026-10-26T11:58:00Z" });
    expect(shown.badge).toHaveTextContent(/^Not evaluated$/);
    expect(shown.now).toHaveTextContent(/^Not evaluated yet: it is, within a minute\.$/);

    shown = await standing({ value: null, evaluated_at: null });
    expect(shown.now).toHaveTextContent(
      /^Changed since its last reading, taken again within a minute; Alert until then\.$/,
    );
    shown = await standing({});
    expect(shown.badge).toHaveTextContent(/^Alert$/);
    expect(shown.now).toHaveTextContent(/^6,200 ms, read \S.*; Alert since \S.*\.$/);
  });

  // ⚠ Under the alert that opened them, open ones first, each with where its notices went.
  it("lists each alert's incidents under it as the server ordered them", async () => {
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const [open, taken, done] = [incidentItem(RULE, 0), incidentItem(RULE, 1), incidentItem(RULE, 2)];
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

  // ⚠ What changed and when, the reader left to judge: the newest first, as the server ordered them.
  it("names under an incident what its alert's agents changed to before it opened", async () => {
    const OTHER_RUN = "ab3e2074-0000-4000-8000-000000000009";
    mockAlerts = {
      ...(mockAlerts as object),
      incidents: [
        incident("i-open", {
          changes: [
            change(),
            change({ kind: "model", value: "gpt-4o-mini", changed_at: "2026-10-26T08:00:00Z", capped: true }),
            change({
              agent: "triage",
              environment: "",
              kind: "prompt",
              prompt: "reply",
              value: "3",
              changed_at: "2026-10-25T11:01:00Z",
              run_id: OTHER_RUN,
            }),
          ],
          changes_more: true,
        }),
        incident("i-quiet"),
      ],
    };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const open = incidentItem(RULE, 0);
    const list = within(open).getByRole("list", { name: "Changed in the 24 hours before it opened" });
    // SLOW reads every agent's model calls, so each change names whose it is.
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "support: release 2026.11.3 in production, 14 minutes before",
      "support: model gpt-4o-mini in production; its later model changes that UTC day are not marked, 3 hours before",
      "triage: prompt reply version 3, 23 hours before",
    ]);
    expect(
      within(list).getByRole("link", { name: "support: release 2026.11.3 in production" }),
    ).toHaveAttribute("href", `/acme/web/traces/${RUN}`);
    expect(within(list).getByRole("link", { name: "triage: prompt reply version 3" })).toHaveAttribute(
      "href",
      `/acme/web/traces/${OTHER_RUN}`,
    );
    expect(open).toHaveTextContent("and more");

    // Nothing changed: nothing is said.
    const quiet = incidentItem(RULE, 1);
    expect(within(quiet).queryByRole("list")).toBeNull();
    expect(quiet).not.toHaveTextContent(/Changed in|and more|couldn/);
  });

  it("leaves a change's agent out under an alert held to one agent", async () => {
    mockAlerts = {
      ...(mockAlerts as object),
      alerts: [{ ...SLOW, filters: { agent: ["support"] } }],
      incidents: [incident("i-open", { changes: [change()] })],
    };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const open = incidentItem(RULE, 0);
    expect(within(open).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "release 2026.11.3 in production, 14 minutes before",
    ]);
    expect(open).not.toHaveTextContent("and more");
  });

  // ⚠ None read is not none changed.
  it("says so where an incident's changes could not be read", async () => {
    mockAlerts = {
      ...(mockAlerts as object),
      incidents: [incident("i-open", { changes: null })],
    };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const open = incidentItem(RULE, 0);
    expect(open).toHaveTextContent("Changes couldn\u2019t be read.");
    expect(within(open).queryByRole("list")).toBeNull();
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

  // ⚠ Marked past WCAG's 3:1 for a state, on the page of either theme, by a
  // fill or a border: the focus the link hands it clears a ring (globals.css).
  it("marks a followed incident past 3:1 against the page, light and dark", async () => {
    const held = "6e5a6d79-0000-4000-8000-00000000000a";
    const alerts = mockAlerts as { incidents: ReturnType<typeof incident>[] };
    alerts.incidents = [incident(held), ...alerts.incidents.slice(1)];
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({ incident: held }) }));
    const followed = incidentItem(RULE, 0);
    for (const theme of [".theme-console", ".dark .theme-console"]) {
      const page = srgb(oklch(theme, "background")!);
      const marks = [...followed.classList].flatMap((name) => {
        const [, token, opacity] = /^(?:bg|border(?:-[trblxy])?)-([a-z-]+?)(?:\/(\d+))?$/.exec(name) ?? [];
        const color = token === undefined ? null : oklch(theme, token);
        if (!color) return [];
        const alpha = opacity ? Number(opacity) / 100 : 1;
        return [contrast(over(srgb(color), page, alpha), page)];
      });
      expect(Math.max(0, ...marks), theme).toBeGreaterThanOrEqual(3);
    }
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
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const row = screen.getByText("Every alert is muted").closest("div")!.parentElement!;
    expect(row).toHaveTextContent("set by Ada");
    expect(within(row).getByTestId("end-mute").dataset.mute).toBe("m-all");
  });

  it("names each starter alert as one, beside its severity", async () => {
    const STARTER = "6c3e2084-0000-4000-8000-000000000009";
    mockAlerts = {
      ...(mockAlerts as object),
      alerts: [
        CAP,
        SLOW,
        {
          ...SLOW,
          id: STARTER,
          name: "Failing runs",
          kind: "run",
          measure: "failures",
          aggregation: "avg",
          filters: {},
          alert_threshold: 0.1,
          warning_threshold: 0.05,
          connectors: [],
          starter: true,
        },
      ],
    };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const starter = document.getElementById(`alert-${STARTER}`)!;
    expect(within(starter).getByText("Starter")).toBeInTheDocument();
    expect(within(starter).getByText("Alert > 10%, warning > 5%")).toBeInTheDocument();
    // Edited and deleted as any alert is, in one click.
    expect(within(starter).getByTestId("alert-actions").dataset.alert).toBe(STARTER);
    expect(within(document.getElementById(`alert-${RULE}`)!).queryByText("Starter")).toBeNull();
  });

  it("connects from the first run itself: Slack's handshake in a new tab, the webhook's dialog in place", async () => {
    mockAlerts = { ...(mockAlerts as object), alerts: [], mutes: [] };
    mockProject = { ...(mockProject as object), role: Role.Admin };
    mockConnectors = { ...(mockConnectors as object), connections: [] };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const slack = screen.getByRole("link", { name: "Connect Slack" });
    expect(slack.getAttribute("href")).toMatch(/^\/connect\/slack\/start\?project=/);
    expect(slack).toHaveAttribute("target", "_blank");
    const webhook = screen.getByRole("button", { name: "Connect a webhook" });
    for (const control of [slack, webhook]) {
      expect(control.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    }
  });

  // A connect refreshes the page; were the control dropped once the step is
  // done, the webhook's once-shown secret would go with it.
  it("keeps offering the connect controls once the step is done, and never to a member", async () => {
    mockAlerts = { ...(mockAlerts as object), alerts: [], mutes: [] };
    mockProject = { ...(mockProject as object), role: Role.Admin };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("heading", { name: /Choose where alerts go.*\(done\)/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect a webhook" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Connect Slack" })).toBeInTheDocument();

    cleanup();
    mockProject = { ...(mockProject as object), role: Role.Member };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByRole("button", { name: "Connect a webhook" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Connect Slack" })).toBeNull();
  });

  it("offers only the connectors this deployment has", async () => {
    mockAlerts = { ...(mockAlerts as object), alerts: [], mutes: [] };
    mockProject = { ...(mockProject as object), role: Role.Admin };
    mockConnectors = { ...(mockConnectors as object), enabled: { slack: false, webhook: true }, connections: [] };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByRole("link", { name: "Connect Slack" })).toBeNull();
    expect(screen.getByRole("button", { name: "Connect a webhook" })).toBeInTheDocument();
  });

  it("names the starters the plan had no room for, in the plan's own words", async () => {
    const plan = "This organization's plan holds 10 alerts across its projects.";
    mockAlerts = { ...(mockAlerts as object), startersWithheld: ["repeats", "stalled"], startersDetail: plan };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const notice = screen.getByText("Not made, past the plan's number: Looping runs and Stalled runs");
    expect(notice.parentElement).toHaveTextContent(
      `This project's first run found no room for these starter alerts. ${plan}`,
    );

    cleanup();
    mockAlerts = { ...(mockAlerts as object), startersWithheld: [], startersDetail: null };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByText(/Not made, past the plan's number/)).toBeNull();
  });

  it("draws a project with no alert as its first run, where its alerts go first", async () => {
    mockAlerts = { ...(mockAlerts as object), alerts: [], mutes: [] };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("heading", { name: "Hear about it before your users do" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Choose where alerts go/ })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Decide what to watch" })).toBeInTheDocument();
    // Nothing to mute: only New alert stands at the page's head.
    expect(screen.queryByTestId("mute")).toBeNull();
    expect(screen.getByTestId("new-alert")).toBeInTheDocument();
  });

  it("names nobody when the roster cannot be read", async () => {
    mockMembers = { kind: "unavailable" };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const row = screen.getByText("Every alert is muted").closest("div")!.parentElement!;
    expect(row).toHaveTextContent("set by someone");
    expect(within(incidentItem(RULE, 1)).getByText("Acknowledged by someone")).toBeInTheDocument();
    expect(fetchOrganizationMembers).not.toHaveBeenCalled();
  });

  // ⚠ An organization's admin acts on every project with no seat on it.
  it("names an admin the project's roster lacks from the organization's, to an admin", async () => {
    const taken = (id: string, by: string) =>
      incident(id, { status: "acknowledged", acknowledged_by: by, acknowledged_at: "2026-10-26T11:10:00Z" });
    mockAlerts = { ...(mockAlerts as object), incidents: [taken("i-admin", "user_admin"), taken("i-gone", "user_gone")] };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchOrganizationMembers).not.toHaveBeenCalled();
    expect(within(incidentItem(RULE, 0)).getByText("Acknowledged by an organization admin or a former member")).toBeInTheDocument();

    cleanup();
    mockOrganization = { organizationId: "org_1", slug: "acme", name: "Acme", role: "admin" };
    mockOrganizationMembers = {
      kind: "ok",
      ceiling: null,
      members: [{ id: "o1", memberId: "user_admin", email: "grace@example.test", displayName: "Grace", role: "admin", createdAt: "" }],
    };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const [admin, gone] = [incidentItem(RULE, 0), incidentItem(RULE, 1)];
    expect(within(admin).getByText("Acknowledged by Grace")).toBeInTheDocument();
    expect(within(gone).getByText("Acknowledged by a former member")).toBeInTheDocument();
  });

  it("calls a connection by no wrong name when the connectors cannot be read", async () => {
    mockConnectors = null;
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const slow = document.getElementById(`alert-${RULE}`)!;
    expect(within(slow).getAllByText(/^A connector/).length).toBe(2);
    expect(within(slow).queryByText(/removed/)).toBeNull();
    expect(incidentItems(RULE)[0]).toHaveTextContent("told the feed, a connector, a connector");
  });

  it("draws the outage and the refusal as every page does", async () => {
    mockAlerts = { kind: "unavailable" };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    cleanup();
    mockAlerts = { kind: "forbidden" };
    render(await AlertsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("denied")).toHaveTextContent("this project's alerts");
  });
});
