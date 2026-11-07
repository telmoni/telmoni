// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { Role } from "@/lib/types/enums";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title }: { title: string }) => (
    <div data-testid="page-header">{title}</div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("./_rename-project", () => ({
  RenameProjectForm: ({
    projectId,
    slug,
    initialName,
    canEdit,
  }: {
    projectId: string;
    slug: string;
    initialName: string;
    canEdit: boolean;
  }) => (
    <div data-testid="rename-form" data-project-id={projectId} data-slug={slug}>
      <span>{initialName}</span>
      <span>{canEdit ? "editable" : "readonly"}</span>
    </div>
  ),
}));

vi.mock("./_content-mode", () => ({
  ContentModeForm: ({
    projectId,
    mode,
    offered,
    canEdit,
  }: {
    projectId: string;
    mode: string;
    offered: readonly string[];
    canEdit: boolean;
  }) => (
    <div
      data-testid="content-mode-form"
      data-project-id={projectId}
      data-mode={mode}
      data-offered={offered.join(",")}
    >
      {canEdit ? "editable" : "readonly"}
    </div>
  ),
}));

vi.mock("./_delete-project", () => ({
  DeleteProjectForm: ({ projectId, projectName }: { projectId: string; projectName: string }) => (
    <div data-testid="delete-form" data-project-id={projectId}>
      {projectName}
    </div>
  ),
}));

vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("notFound");
  },
  useRouter: () => ({ refresh: () => {} }),
}));

let mockContext: unknown;
let mockProject: unknown;
let mockContentMode: unknown;
let mockPosture: unknown;
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
  fetchContentMode: async () => mockContentMode,
  fetchModelPrices: async () => [],
  fetchPrivacyPosture: async () => mockPosture,
}));

let mockLimits: unknown = null;
vi.mock("@/lib/server/entities/limits", () => ({
  fetchLimits: async () => mockLimits,
}));

vi.mock("@/lib/server/session", () => ({
  getServerSession: async () => ({
    userId: "user_1",
    email: "k@example.com",
    firstName: "K",
    lastName: null,
    sessionRowId: null,
  }),
}));

import SettingsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "my-project" });

beforeEach(() => {
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test" },
    organizations: [
      {
        organizationId: "org_1",
        slug: "acme",
        name: "Acme",
        ownerEmail: "ada@example.test",
        role: "owner",
      },
    ],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = {
    id: "project_7bQx2mNv9BcK4dLp",
    slug: "my-project",
    name: "My Project",
    role: Role.Owner,
  };
  mockContentMode = { content_mode: "off", offered: ["off"] };
  mockPosture = { retention: { span_days: 90, max_span_days: 360, rollup_months: 13 } };
  mockLimits = null;
});

// The project's own caps, read where its members work, set where its
// organization's owners and admins divide the pool.
describe("SettingsPage: the project's limits", () => {
  const page = async () => render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
  const none = { spans: null, content_bytes: null, alerts: null, monitors: null, objectives: null, alert_deliveries_per_day: null };

  it("shows each cap, a limit with none sharing the pool, and where they are set", async () => {
    mockLimits = {
      plan: { spans: null, content_bytes: null, alerts: null, alert_deliveries_per_day: null },
      projects: [
        {
          id: "project_7bQx2mNv9BcK4dLp",
          name: "My Project",
          caps: { ...none, spans: 200_000, content_bytes: 5_000_000 },
          used: { spans: 1, content_bytes: 0, alerts: 0, monitors: 0, objectives: 0, alert_deliveries_today: null },
        },
      ],
    };
    await page();
    const row = (name: string) => screen.getByRole("cell", { name }).closest("tr");
    expect(row("Spans a month")).toHaveTextContent("200,000");
    expect(row("Content held, MB")).toHaveTextContent("5");
    expect(row("Alerts")).toHaveTextContent("Shares the pool");
    expect(screen.getByRole("link", { name: "Limits" })).toHaveAttribute("href", "/acme/limits");
  });

  it("says when the limits could not be read", async () => {
    await page();
    expect(screen.getByText("The limits could not be read. Reload to try again.")).toBeInTheDocument();
  });
});

describe("SettingsPage: the end user's data and its retention", () => {
  const page = async () => render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));

  it("offers an admin the erasure of an end user, and tells a member who erases one", async () => {
    mockProject = { ...(mockProject as object), role: Role.Member };
    const { unmount } = await page();
    expect(screen.queryByRole("textbox", { name: "End user id" })).not.toBeInTheDocument();
    expect(screen.getByText("A project admin or the organization owner erases an end user.")).toBeInTheDocument();
    unmount();

    mockProject = { ...(mockProject as object), role: Role.Admin };
    await page();
    expect(screen.getByRole("textbox", { name: "End user id" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Erase" })).toBeDisabled();
  });

  it("tells anyone how an end user is given their data", async () => {
    await page();
    expect(screen.getByRole("link", { name: "Traces" })).toHaveAttribute("href", "/acme/my-project/traces");
  });

  // ⚠ The project's own retention is its owner's or an admin's to choose,
  // up to the longest offered; a member reads it alone.
  it("offers an admin the retention to choose, and a member the days alone", async () => {
    mockProject = { ...(mockProject as object), role: Role.Member };
    const { unmount } = await page();
    expect(screen.getByText("90 days after a span arrives")).toBeInTheDocument();
    expect(screen.getByText("13 months")).toBeInTheDocument();
    const tableScroll = screen.getByRole("columnheader", { name: "Kept" }).closest('[data-slot="table-scroll"]');
    expect(tableScroll).toHaveAttribute("tabindex", "0");
    expect(tableScroll?.closest('[data-slot="card"]')?.querySelector('[data-slot="card"]')).toBeNull();
    expect(screen.queryByLabelText("Keep spans for")).not.toBeInTheDocument();
    unmount();

    mockProject = { ...(mockProject as object), role: Role.Admin };
    await page();
    const field = screen.getByLabelText("Keep spans for");
    expect(field).toHaveValue(90);
    expect(field).toHaveAttribute("max", "360");
  });

  it("says so when the retention cannot be read, and keeps the rest", async () => {
    mockPosture = null;
    await page();
    expect(screen.getByText(/^Retention could not be read\./)).toBeInTheDocument();
    expect(screen.getByTestId("rename-form")).toBeInTheDocument();
  });
});

describe("SettingsPage", () => {
  describe("project mode", () => {
    it("renders the project settings and rename form", async () => {
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("My Project");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("editable");
    });

    // The id is what an API request and an SDK are configured with; the slug
    // in the path follows the name and is nothing to copy.
    it("shows the project's id, and hands the rename the id and the path's slug", async () => {
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.getByText("project_7bQx2mNv9BcK4dLp")).toBeInTheDocument();
      const form = screen.getByTestId("rename-form");
      expect(form).toHaveAttribute("data-project-id", "project_7bQx2mNv9BcK4dLp");
      expect(form).toHaveAttribute("data-slug", "my-project");
    });

    it("offers the owner a danger zone for this project, named", async () => {
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      const form = screen.getByTestId("delete-form");
      expect(form).toHaveAttribute("data-project-id", "project_7bQx2mNv9BcK4dLp");
      expect(form).toHaveTextContent("My Project");
      const headings = screen
        .getAllByRole("heading", { level: 2 })
        .map((h) => h.textContent?.toLowerCase() ?? "");
      expect(headings[headings.length - 1]).toMatch(/danger zone/i);
    });

    it("draws no danger zone for an admin, but allows editable name", async () => {
      mockProject = {
        id: "project_7bQx2mNv9BcK4dLp",
        name: "My Project",
        role: Role.Admin,
      };
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
      expect(screen.getByTestId("rename-form")).toHaveTextContent("editable");
    });

    it("draws no danger zone for a member, and readonly name", async () => {
      mockProject = {
        id: "project_7bQx2mNv9BcK4dLp",
        name: "My Project",
        role: Role.Member,
      };
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
      expect(screen.getByTestId("rename-form")).toHaveTextContent("readonly");
    });

    it("shows outage when context is unavailable", async () => {
      mockContext = null;
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.getByTestId("outage")).toBeInTheDocument();
    });
  });

  // ⚠ What a project keeps changes what its organization is liable for: the
  // organization's owner sets it and nobody below, and every role sees it.
  describe("content mode", () => {
    it("lets the owner change it, with the mode and the modes the server offers", async () => {
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      const form = screen.getByTestId("content-mode-form");
      expect(form).toHaveAttribute("data-project-id", "project_7bQx2mNv9BcK4dLp");
      expect(form).toHaveAttribute("data-mode", "off");
      expect(form).toHaveAttribute("data-offered", "off");
      expect(form).toHaveTextContent("editable");
    });

    it("shows an admin and a member the mode, and lets neither change it", async () => {
      for (const role of [Role.Admin, Role.Member]) {
        mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "my-project", name: "My Project", role };
        const { unmount } = render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
        expect(screen.getByTestId("content-mode-form")).toHaveTextContent("readonly");
        unmount();
      }
    });

    // An outage of the mode's read is not the page's: the rest of the
    // settings still render.
    it("says the mode could not be read, and still draws the rest", async () => {
      mockContentMode = null;
      render(await SettingsPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
      expect(screen.queryByTestId("content-mode-form")).toBeNull();
      expect(screen.getByText(/content mode could not be read/i)).toBeInTheDocument();
      expect(screen.getByTestId("rename-form")).toBeInTheDocument();
    });
  });
});
