// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { IdentityContext } from "@/lib/server/entities/identity-context";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

const HEAD = "9f".repeat(32);
const PLATFORM = "project_platform";
const QUIET = "project_quiet";

let mockContext: unknown;
let mockIdentity: IdentityContext | null = null;
let mockDrawn: unknown;
let mockInventory: unknown;
let draws = 0;

const ROW = (attribute: string, needs: "any" | "on", person: string) => ({
  attribute,
  on: "any span",
  kept: needs === "on" ? "sealed in Postgres alone" : "a column",
  needs,
  rule: "a rule",
  person,
});

vi.mock("@/lib/server/data", async () => {
  const { activeOrganization } = await vi.importActual<
    typeof import("@/lib/server/entities/organization")
  >("@/lib/server/entities/organization");
  return {
    activeOrganization,
    getServerContext: async () => mockContext,
    identityContext: async () => mockIdentity,
    drawDataSheet: async () => {
      draws += 1;
      return mockDrawn;
    },
    fetchProjectListing: async () => ({
      kind: "ok",
      projects: [
        { id: PLATFORM, slug: "platform", name: "Platform" },
        { id: QUIET, slug: "quiet", name: "Quiet" },
      ],
    }),
    fetchOrganizationAuditSince: async () => ({
      kind: "ok",
      complete: true,
      events: [
        {
          id: "r1",
          actorId: "user_ada",
          action: "viewed",
          resourceKind: "project",
          resourceId: PLATFORM,
          createdAt: "2026-11-04T10:00:00Z",
          metadata: { trace_ids: ["t1"], fields: 2 },
          inProject: PLATFORM,
        },
        {
          id: "r2",
          actorId: "key:k_01",
          action: "viewed",
          resourceKind: "project",
          resourceId: PLATFORM,
          createdAt: "2026-11-03T10:00:00Z",
          metadata: { trace_ids: ["t2"], fields: 1 },
          inProject: PLATFORM,
        },
      ],
    }),
    fetchOrganizationMembers: async () => ({
      kind: "ok",
      ceiling: null,
      members: [
        { id: "m1", memberId: "user_ada", email: "ada@example.test", displayName: "Ada", role: "owner", createdAt: "" },
      ],
    }),
    fetchContentMode: async (projectId: string) => ({
      content_mode: projectId === PLATFORM ? "on" : "off",
      offered: ["off", "on"],
    }),
    fetchPrivacyPosture: async (projectId: string) => ({
      days: 30,
      since: "2026-10-06T00:00:00Z",
      spans: 400,
      sent: projectId === PLATFORM ? [{ attribute: "telmoni.scrubbed", spans: 12 }] : [],
      retention: { span_days: 30, max_span_days: 360, rollup_months: 13 },
    }),
    fetchAuditEvents: async (projectId: string) => ({
      kind: "ok",
      events:
        projectId === PLATFORM
          ? [
              {
                id: "c1",
                actorId: "user_ada",
                action: "updated",
                resourceKind: "project",
                resourceId: PLATFORM,
                createdAt: "2026-11-01T09:00:00Z",
                metadata: { content_mode: { from: "off", to: "on" } },
                inProject: PLATFORM,
              },
              {
                id: "c2",
                actorId: "user_ada",
                action: "updated",
                resourceKind: "project",
                resourceId: PLATFORM,
                createdAt: "2026-11-02T09:00:00Z",
                metadata: { retention_days: { from: 30, to: 14 } },
                inProject: PLATFORM,
              },
            ]
          : [],
    }),
    fetchFieldInventory: async () => mockInventory,
  };
});

import DataSheetPage from "./page";

function standingAs(role: IdentityContext["role"]) {
  mockIdentity = { userId: "user_ada", organizationId: "org_acme", role, accessToken: "at_1" };
  mockContext = {
    organizations: [
      { organizationId: "org_acme", slug: "acme", name: "Acme", ownerEmail: "ada@example.test", role },
    ],
    activeOrganizationId: "org_acme",
    memberships: [],
    incomingInvites: [],
  };
}

beforeEach(() => {
  draws = 0;
  standingAs("owner");
  mockDrawn = { kind: "ok", drawn: { seq: 42, rowHash: HEAD, drawnAt: "2026-11-05T12:00:00Z" } };
  mockInventory = {
    conventions_commit: "a1b2c3d",
    rows: [
      ROW("gen_ai.request.model", "any", "no"),
      ROW("user.id", "any", "hashed"),
      ROW("gen_ai.input.messages", "on", "content"),
    ],
  };
});

describe("DataSheetPage", () => {
  it("draws each project's mode, its history, its retention and its readers, with the chain's head", async () => {
    render(await DataSheetPage());
    expect(draws).toBe(1);
    expect(screen.getByText(HEAD)).toBeInTheDocument();
    expect(screen.getByText(`node verify-audit.mjs export.json ${HEAD}`)).toBeInTheDocument();
    expect(screen.getByText(/This drawing is row 42/)).toBeInTheDocument();

    const platform = screen.getByRole("link", { name: "Platform" }).closest("[data-slot=card]") as HTMLElement;
    expect(within(platform).getByText("On: content kept, sealed")).toBeInTheDocument();
    expect(within(platform).getByText(/12 of its 400 spans of the last 30 days arrived with something scrubbed/)).toBeInTheDocument();
    // The mode's one change, its retention's change passed over.
    expect(within(platform).getAllByRole("row")).toHaveLength(1 + 1 + 1 + 2);
    expect(within(platform).getByText("API key k_01")).toBeInTheDocument();
    expect(within(platform).getAllByText("Ada").length).toBeGreaterThan(0);

    const quiet = screen.getByRole("link", { name: "Quiet" }).closest("[data-slot=card]") as HTMLElement;
    expect(within(quiet).getByText("Off: content dropped at the door")).toBeInTheDocument();
    expect(within(quiet).getByText(/never been changed/)).toBeInTheDocument();
    expect(within(quiet).getByText(/Nobody read its content in the last 90 days/)).toBeInTheDocument();
    expect(within(quiet).getByText(/No span of the last 30 days arrived with anything scrubbed/)).toBeInTheDocument();

    expect(screen.getByText("Only as the project's hash of it")).toBeInTheDocument();
    expect(screen.getByText("Yes: it is content")).toBeInTheDocument();
    expect(screen.getByText(/aws key|AWS key/)).toBeInTheDocument();
    expect(screen.getByText(/This deployment names none/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Print" })).toHaveAttribute("data-print", "hide");
  });

  it("draws a field the inventory gains with no other change", async () => {
    mockInventory = {
      conventions_commit: "a1b2c3d",
      rows: [
        ...(mockInventory as { rows: unknown[] }).rows,
        ROW("telmoni.new_field", "any", "if_sent"),
      ],
    };
    render(await DataSheetPage());
    expect(screen.getByText("telmoni.new_field")).toBeInTheDocument();
    expect(screen.getByText("Only if your code puts one there")).toBeInTheDocument();
  });

  it("is its owners' and admins' alone, and files no read for anyone else", async () => {
    standingAs("member");
    render(await DataSheetPage());
    expect(screen.getByText(/this organization's data sheet/i)).toBeInTheDocument();
    expect(draws).toBe(0);
  });

  it("is an outage where the read could not be filed", async () => {
    mockDrawn = { kind: "unavailable" };
    render(await DataSheetPage());
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
