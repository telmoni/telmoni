// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConnectorsPage from "./page";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => <h1>{title}</h1>,
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="service-unavailable" />,
}));

vi.mock("./_manage", () => ({
  ConnectionActions: ({ connection }: { connection: { id: string } }) => (
    <div data-testid={`actions-${connection.id}`} />
  ),
}));

vi.mock("./_deliveries", () => ({
  DeliveriesButton: ({ connectionId, canResend }: { connectionId: string; canResend: boolean }) => (
    <div data-testid={`deliveries-${connectionId}`} data-can-resend={String(canResend)} />
  ),
}));

vi.mock("./_webhook", () => ({
  ConnectWebhookButton: ({ label }: { label: string }) => <button type="button">{label}</button>,
}));

let mockContext: unknown;
let mockProject: unknown;
let mockListing: unknown;

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchProject: async () => mockProject,
  fetchConnectors: async () => mockListing,
}));

const PROJECT = "project_abc";

function connection(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    id: "11111111-1111-4111-8111-111111111111",
    provider: "slack",
    external_workspace_id: "T0001",
    external_workspace_name: "Acme",
    channel_id: "C0ALERTS",
    channel_name: "alerts",
    status: "active",
    last_error: null,
    last_delivery_at: null,
    created_at: "2026-09-20T12:00:00Z",
    event_kinds: null,
    previous_secret_expires_at: null,
    ...overrides,
  };
}

function webhook(overrides: Partial<Record<string, unknown>> = {}) {
  return connection({
    id: "33333333-3333-4333-8333-333333333333",
    provider: "webhook",
    external_workspace_id: "hooks.example.com",
    external_workspace_name: null,
    channel_id: "3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f",
    channel_name: "hooks.example.com",
    ...overrides,
  });
}

async function renderPage(search: { connected?: string; error?: string } = {}) {
  render(
    await ConnectorsPage({
      params: Promise.resolve({ projectId: PROJECT }),
      searchParams: Promise.resolve(search),
    }),
  );
}

beforeEach(() => {
  mockContext = { organization: { organizationId: "org_1" } };
  mockProject = { id: PROJECT, name: "Project", role: "owner" };
  mockListing = {
    enabled: { slack: true, discord: false, webhook: true },
    connections: [],
  };
});

describe("ConnectorsPage", () => {
  it("names the page and offers the three destinations", async () => {
    await renderPage();
    expect(screen.getByRole("heading", { name: "Connectors" })).toBeInTheDocument();
    expect(screen.getByText("Slack")).toBeInTheDocument();
    expect(screen.getByText("Discord")).toBeInTheDocument();
    expect(screen.getByText("Webhook")).toBeInTheDocument();
  });

  // The webhook has no handshake to link to: its Connect is a dialog button,
  // never an anchor to `/connect/webhook/start`.
  it("gives the owner a dialog button for the webhook, not a link", async () => {
    await renderPage();
    expect(screen.getByRole("button", { name: "Connect Webhook" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Webhook/ })).toBeNull();
  });

  // The link is a plain anchor to the GET Route Handler that starts the
  // handshake, and it names the project — the start route reads it back.
  it("gives the owner a Connect link for an enabled provider", async () => {
    await renderPage();
    const link = screen.getByRole("link", { name: "Connect Slack" });
    expect(link).toHaveAttribute("href", `/connect/slack/start?project=${PROJECT}`);
  });

  // A provider this deployment holds no credentials for renders exactly as
  // the page did before the lane existed: a Soon badge and a disabled button.
  it("keeps a provider that is not configured disabled and labelled", async () => {
    await renderPage();
    expect(screen.getByText("Soon")).toBeInTheDocument();
    const discord = screen.getByRole("button", { name: /^Connect Discord/ });
    expect(discord).toBeDisabled();
    expect(screen.queryByRole("link", { name: /Discord/ })).toBeNull();
  });

  it("tells a member who can connect, and offers no link or dialog", async () => {
    mockProject = { id: PROJECT, name: "Project", role: "member" };
    await renderPage();
    expect(screen.queryByRole("link", { name: /^Connect/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Connect Webhook" })).toBeNull();
    // Once per enabled tile: Slack and the webhook; Discord is disabled.
    expect(
      screen.getAllByText("Only a project owner or admin can connect or remove a destination."),
    ).toHaveLength(2);
  });

  it("gives an admin a Connect link for an enabled provider and dialog for webhook", async () => {
    mockProject = { id: PROJECT, name: "Project", role: "admin" };
    await renderPage();
    const link = screen.getByRole("link", { name: "Connect Slack" });
    expect(link).toHaveAttribute("href", `/connect/slack/start?project=${PROJECT}`);
    expect(screen.getByRole("button", { name: "Connect Webhook" })).toBeInTheDocument();
  });

  // An endpoint shows as its host and the start of its fingerprint — never
  // the URL, which the service seals and the listing does not carry.
  it("lists an endpoint by its host and fingerprint and offers another", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [
        connection({
          id: "33333333-3333-4333-8333-333333333333",
          provider: "webhook",
          external_workspace_id: "hooks.example.com",
          external_workspace_name: null,
          channel_id: "3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f3f9a2c1d7e6b5a4f",
          channel_name: "hooks.example.com",
        }),
      ],
    };
    await renderPage();
    expect(screen.getByText("hooks.example.com")).toBeInTheDocument();
    expect(screen.getByText("3f9a2c1d")).toBeInTheDocument();
    expect(screen.getByTestId("actions-33333333-3333-4333-8333-333333333333")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Connect another endpoint" }),
    ).toBeInTheDocument();
  });

  it("lists a connection by its channel and workspace, with actions for the owner", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [
        connection(),
        connection({
          id: "22222222-2222-4222-8222-222222222222",
          provider: "discord",
          external_workspace_id: "G0",
          external_workspace_name: null,
          channel_name: "Telmoni alerts",
          status: "errored",
          last_error: "discord 10015: Unknown Webhook",
        }),
      ],
    };
    await renderPage();
    expect(screen.getByText("#alerts in Acme")).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
    expect(screen.getByText("No messages yet.")).toBeInTheDocument();
    expect(screen.getByText("Telmoni alerts")).toBeInTheDocument();
    expect(screen.getByText("Needs attention")).toBeInTheDocument();
    expect(screen.getByText("discord 10015: Unknown Webhook")).toBeInTheDocument();
    expect(screen.getByTestId("actions-11111111-1111-4111-8111-111111111111")).toBeInTheDocument();
    expect(screen.getByTestId("actions-22222222-2222-4222-8222-222222222222")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Connect another Slack channel" }),
    ).toHaveAttribute("href", `/connect/slack/start?project=${PROJECT}`);
  });

  it("shows a member the list without actions", async () => {
    mockProject = { id: PROJECT, name: "Project", role: "member" };
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [connection()],
    };
    await renderPage();
    expect(screen.getByText("#alerts in Acme")).toBeInTheDocument();
    expect(screen.queryByTestId(/^actions-/)).toBeNull();
  });

  // The delivery log is a read: every role gets it. Only an owner's copy
  // offers the resend, and only on a webhook, whose receiver deduplicates.
  it("offers the delivery log to a member, without the resend", async () => {
    mockProject = { id: PROJECT, name: "Project", role: "member" };
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [connection({ provider: "webhook" })],
    };
    await renderPage();
    expect(
      screen.getByTestId("deliveries-11111111-1111-4111-8111-111111111111"),
    ).toHaveAttribute("data-can-resend", "false");
  });

  it("offers the owner the delivery log with the resend on a webhook", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [connection({ provider: "webhook" })],
    };
    await renderPage();
    expect(
      screen.getByTestId("deliveries-11111111-1111-4111-8111-111111111111"),
    ).toHaveAttribute("data-can-resend", "true");
  });

  it("offers no resend on a chat channel, even to the owner", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [connection()],
    };
    await renderPage();
    expect(
      screen.getByTestId("deliveries-11111111-1111-4111-8111-111111111111"),
    ).toHaveAttribute("data-can-resend", "false");
  });

  it("says a webhook with no filter receives all events", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [webhook()],
    };
    await renderPage();
    expect(screen.getByText("Events: All events")).toBeInTheDocument();
  });

  it("names up to two chosen events by their labels", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [webhook({ event_kinds: ["member_added", "connector_connected"] })],
    };
    await renderPage();
    expect(screen.getByText("Events: Member added, Channel connected")).toBeInTheDocument();
  });

  it("counts more than two chosen events", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [
        webhook({
          event_kinds: ["member_added", "connector_connected", "connector_disconnected"],
        }),
      ],
    };
    await renderPage();
    expect(screen.getByText("Events: 3 events")).toBeInTheDocument();
  });

  // Slack and Discord take every kind; the line is the webhook's alone.
  it("gives a vendor channel no events line", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [connection()],
    };
    await renderPage();
    expect(screen.queryByText(/^Events:/)).toBeNull();
  });

  it("shows when a rotated webhook's previous secret stops", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [webhook({ previous_secret_expires_at: "2026-09-21T12:00:00Z" })],
    };
    await renderPage();
    const line = screen.getByText(/Previous secret valid until/);
    expect(line.querySelector("time")).toHaveAttribute("dateTime", "2026-09-21T12:00:00Z");
  });

  it("shows no previous-secret line outside a rotation", async () => {
    mockListing = {
      enabled: { slack: true, discord: true, webhook: true },
      connections: [webhook()],
    };
    await renderPage();
    expect(screen.queryByText(/Previous secret valid until/)).toBeNull();
  });

  // No listing at all — the server is down — is the pre-lane page: every
  // tile labelled and disabled.
  it("renders every tile as unavailable when the listing cannot be read", async () => {
    mockListing = null;
    await renderPage();
    expect(screen.getAllByText("Soon")).toHaveLength(3);
    const connect = screen.getAllByRole("button", { name: /^Connect / });
    expect(connect).toHaveLength(3);
    for (const button of connect) expect(button).toBeDisabled();
    expect(screen.queryByRole("link", { name: /^Connect/ })).toBeNull();
  });

  it("reports what the handshake came back with", async () => {
    await renderPage({ connected: "slack" });
    expect(screen.getByRole("status")).toHaveTextContent("Slack is connected.");
  });

  it("names a cancelled or expired handshake in a sentence", async () => {
    await renderPage({ error: "denied" });
    expect(screen.getByRole("status")).toHaveTextContent("cancelled at the vendor's page");
  });

  it("renders the outage card when the project cannot be read", async () => {
    mockProject = null;
    await renderPage();
    expect(screen.getByTestId("service-unavailable")).toBeInTheDocument();
  });

  it("renders the outage card with no server context", async () => {
    mockContext = null;
    await renderPage();
    expect(screen.getByTestId("service-unavailable")).toBeInTheDocument();
  });
});
