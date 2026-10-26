// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, backHref }: { title: string; backHref?: string }) => (
    <div data-testid="page-header" data-back={backHref}>
      {title}
    </div>
  ),
}));
vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const voteIncidentAction = vi.fn<
  (projectId: string, incidentId: string, verdict: string) => Promise<{ error: string | null }>
>(async () => ({ error: null }));
vi.mock("./actions", () => ({
  voteIncidentAction: (projectId: string, incidentId: string, verdict: string) =>
    voteIncidentAction(projectId, incidentId, verdict),
}));

let mockAccess: unknown;
const fetchIncident = vi.fn<(projectId: string, incidentId: string) => Promise<unknown>>(
  async () => mockAccess,
);
vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => ({ memberships: [] }),
  fetchProjectBySlug: async () => ({ id: "project_1", slug: "web" }),
  fetchIncident: (projectId: string, incidentId: string) => fetchIncident(projectId, incidentId),
}));

import IncidentVotePage from "./page";

const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";
const PARAMS = Promise.resolve({ organization: "acme", project: "web", id: INCIDENT });
const OPEN = {
  id: INCIDENT,
  rule_name: "Slow calls",
  status: "open",
  severity: "alert",
  opened_at: "2026-10-26T03:00:00Z",
  resolved_at: null,
  useful: 2,
  noise: 1,
  yours: null,
};

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockAccess = { kind: "ok", incident: OPEN };
});

describe("IncidentVotePage", () => {
  // A mail scanner or a chat's preview opens the link too.
  it("shows the incident and its votes, and opening it casts nothing", async () => {
    render(await IncidentVotePage({ params: PARAMS }));
    expect(fetchIncident).toHaveBeenCalledWith("project_1", INCIDENT);
    expect(screen.getByTestId("page-header").textContent).toBe("Slow calls");
    expect(screen.getByTestId("page-header").dataset.back).toBe("/acme/web/alerts");
    expect(screen.getByText(/2 useful, 1 noise so far/)).toBeTruthy();
    expect(voteIncidentAction).not.toHaveBeenCalled();
  });

  it("shows the person's own word as theirs, and posts the one they pick", async () => {
    mockAccess = { kind: "ok", incident: { ...OPEN, yours: "useful" } };
    render(await IncidentVotePage({ params: PARAMS }));
    expect(screen.getByRole("button", { name: "Useful" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    expect(screen.getByRole("button", { name: "Noise" }).getAttribute("aria-pressed")).toBe(
      "false",
    );
    fireEvent.click(screen.getByRole("button", { name: "Noise" }));
    await waitFor(() =>
      expect(voteIncidentAction).toHaveBeenCalledWith("project_1", INCIDENT, "noise"),
    );
  });

  it("says what the service refused a vote for", async () => {
    voteIncidentAction.mockResolvedValueOnce({ error: "Too many requests — slow down a moment." });
    render(await IncidentVotePage({ params: PARAMS }));
    fireEvent.click(screen.getByRole("button", { name: "Useful" }));
    expect(await screen.findByText("Too many requests — slow down a moment.")).toBeTruthy();
  });

  it("says an incident the project does not have is not here, and an outage is one", async () => {
    mockAccess = { kind: "missing" };
    render(await IncidentVotePage({ params: PARAMS }));
    expect(screen.getByText("No incident with this id in this project.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Useful" })).toBeNull();

    cleanup();
    mockAccess = { kind: "unavailable" };
    render(await IncidentVotePage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeTruthy();
  });
});
