// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => <div data-testid="page-header">{title}</div>,
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockContentMode: unknown;
let mockInventory: unknown;
let mockPosture: unknown;
let mockReads: unknown;
let mockMembers: unknown;
let mockOrganization: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchContentMode = vi.fn<(projectId: string) => Promise<unknown>>(
  async () => mockContentMode,
);
const fetchFieldInventory = vi.fn<(projectId: string) => Promise<unknown>>(
  async () => mockInventory,
);

const fetchAuditEvents = vi.fn<(...args: unknown[]) => Promise<unknown>>(async () => mockReads);

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  activeOrganization: () => mockOrganization,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchContentMode: (projectId: string) => fetchContentMode(projectId),
  fetchFieldInventory: (projectId: string) => fetchFieldInventory(projectId),
  fetchPrivacyPosture: async () => mockPosture,
  fetchAuditEvents: (...args: unknown[]) => fetchAuditEvents(...args),
  fetchMembers: async () => mockMembers,
  fetchOrganizationMembers: async () => ({ kind: "forbidden" }),
}));

import PrivacyPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "my-project" });
const COMMIT = "6fd0d763a092db245a54104fc1124031eb3c51d2";

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
  mockProject = {
    id: "project_7bQx2mNv9BcK4dLp",
    slug: "my-project",
    name: "My Project",
    role: Role.Member,
  };
  mockContentMode = { content_mode: "off", offered: ["off"] };
  mockOrganization = { id: "org_1", role: "member" };
  mockPosture = {
    days: 30,
    since: "2026-09-30T10:00:00Z",
    spans: 1200,
    sent: [{ attribute: "user.email", spans: 400 }],
    retention: { span_days: 360, rollup_months: 13 },
  };
  mockReads = {
    kind: "ok",
    events: [
      {
        id: "e1",
        actorId: "user_1",
        action: "viewed",
        resourceKind: "project",
        resourceId: "project_7bQx2mNv9BcK4dLp",
        createdAt: "2026-10-29T10:00:00Z",
        metadata: { trace_ids: ["t1", "t2"], fields: 4 },
      },
    ],
  };
  mockMembers = {
    kind: "ok",
    members: [{ memberId: "user_1", displayName: "Ada", email: "ada@example.test" }],
    ceiling: null,
  };
  mockInventory = {
    conventions_commit: COMMIT,
    rows: [
      {
        attribute: "telmoni.run_id",
        on: "every span",
        kept: "the column run_id",
        needs: "any",
        rule: "a UUID, the same on every span of a run",
      },
      {
        attribute: "gen_ai.input.messages",
        on: "any span",
        kept: "dropped at the door on a project whose content mode is off",
        needs: "on",
        rule: "content: what a person or a model wrote",
      },
      {
        attribute: "any other text or array",
        on: "any span",
        kept: "dropped in every mode",
        needs: "any",
        rule: "content arrives under names no convention gave it",
      },
    ],
  };
});

describe("PrivacyPage", () => {
  it("shows the project's content mode, and where it is changed", async () => {
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Privacy");
    expect(screen.getByText("Off")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute(
      "href",
      "/acme/my-project/settings",
    );
  });

  it("lists every row the server holds, by the project's id, with the pinned commit", async () => {
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchProjectBySlug).toHaveBeenCalledWith("my-project");
    expect(fetchFieldInventory).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp");
    expect(fetchContentMode).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp");
    expect(screen.getByText("telmoni.run_id")).toBeInTheDocument();
    expect(screen.getByText("gen_ai.input.messages")).toBeInTheDocument();
    expect(screen.getByText("any other text or array")).toBeInTheDocument();
    expect(screen.getAllByText("on").length).toBeGreaterThan(0);
    expect(screen.getByText(COMMIT)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "docs" })).toHaveAttribute(
      "href",
      "/docs/api/field-inventory",
    );
  });

  // A member reads the page as an owner does: the inventory is the same list
  // for every project, and the mode is every role's to read.
  it("says so when the inventory or the mode cannot be read, and keeps the rest", async () => {
    mockInventory = null;
    mockContentMode = null;
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("The inventory could not be read.")).toBeInTheDocument();
    expect(
      screen.getByText("This project's content mode could not be read. Reload to try again."),
    ).toBeInTheDocument();
    expect(screen.getByTestId("page-header")).toHaveTextContent("Privacy");
  });

  it("offers an admin the erasure of an end user, and tells a member who erases one", async () => {
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.queryByRole("textbox", { name: "End user id" })).not.toBeInTheDocument();
    expect(
      screen.getByText("A project admin or the organization owner erases an end user."),
    ).toBeInTheDocument();

    mockProject = { ...(mockProject as object), role: Role.Admin };
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByRole("textbox", { name: "End user id" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Erase" })).toBeDisabled();
  });

  it("counts what arrived by the inventory's rows, and how long it is kept", async () => {
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByText("user.email")).toBeInTheDocument();
    expect(screen.getByText("400")).toBeInTheDocument();
    expect(screen.getByText(/Of 1,200 spans/)).toBeInTheDocument();
    expect(screen.getByText("360 days after a span arrives")).toBeInTheDocument();
    expect(screen.getByText("13 months")).toBeInTheDocument();
    expect(screen.getByText(/In this deployment's own stores/)).toBeInTheDocument();
  });

  // ⚠ The audit log is an admin's: a member is told who sees the reads,
  // and nothing of them is asked for.
  it("shows an admin who read content over the posture's window, by name", async () => {
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(fetchAuditEvents).not.toHaveBeenCalled();
    expect(
      screen.getByText("A project admin or the organization owner sees who read content."),
    ).toBeInTheDocument();

    mockProject = { ...(mockProject as object), role: Role.Admin };
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    const [projectId, limit, filter] = fetchAuditEvents.mock.calls[0]!;
    expect([projectId, limit]).toEqual(["project_7bQx2mNv9BcK4dLp", 1000]);
    expect(filter).toEqual({
      action: "viewed",
      resourceKind: "project",
      from: "2026-09-30T10:00:00Z",
    });
    expect(screen.getByText("Ada")).toBeInTheDocument();
  });

  it("says so when the posture cannot be read, and keeps the rest", async () => {
    mockPosture = null;
    mockProject = { ...(mockProject as object), role: Role.Admin };
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    for (const notice of [
      "What arrived could not be read.",
      "Retention could not be read.",
      "Who read content could not be read.",
    ]) {
      expect(screen.getByText(notice)).toBeInTheDocument();
    }
    expect(fetchAuditEvents).not.toHaveBeenCalled();
    expect(screen.getByText("telmoni.run_id")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
