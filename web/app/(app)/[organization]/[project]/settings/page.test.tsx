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
    organization,
    slug,
    initialName,
    canEdit,
  }: {
    projectId: string;
    organization: string;
    slug: string;
    initialName: string;
    canEdit: boolean;
  }) => (
    <div data-testid="rename-form" data-project-id={projectId} data-path={`/${organization}/${slug}`}>
      <span>{initialName}</span>
      <span>{canEdit ? "editable" : "readonly"}</span>
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
vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchProjectBySlug: async () => mockProject,
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
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [
      {
        organizationId: "org_1",
        slug: "acme",
        name: null,
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
});

describe("SettingsPage", () => {
  describe("project mode", () => {
    it("renders the project settings and rename form", async () => {
      render(await SettingsPage({ params: PARAMS }));
      expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("My Project");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("editable");
    });

    // The id is what an API request and an SDK are configured with; the slug
    // in the path follows the name and is nothing to copy.
    it("shows the project's id, and hands the rename the id and the path's slugs", async () => {
      render(await SettingsPage({ params: PARAMS }));
      expect(screen.getByText("project_7bQx2mNv9BcK4dLp")).toBeInTheDocument();
      const form = screen.getByTestId("rename-form");
      expect(form).toHaveAttribute("data-project-id", "project_7bQx2mNv9BcK4dLp");
      expect(form).toHaveAttribute("data-path", "/acme/my-project");
    });

    it("offers the owner a danger zone for this project, named", async () => {
      render(await SettingsPage({ params: PARAMS }));
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
      render(await SettingsPage({ params: PARAMS }));
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
      render(await SettingsPage({ params: PARAMS }));
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
      expect(screen.getByTestId("rename-form")).toHaveTextContent("readonly");
    });

    it("shows outage when context is unavailable", async () => {
      mockContext = null;
      render(await SettingsPage({ params: PARAMS }));
      expect(screen.getByTestId("outage")).toBeInTheDocument();
    });
  });
});
