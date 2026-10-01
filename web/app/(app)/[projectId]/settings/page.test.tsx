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
    initialName,
    canEdit,
  }: {
    initialName: string;
    canEdit: boolean;
  }) => (
    <div data-testid="rename-form">
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
  fetchProject: async () => mockProject,
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

beforeEach(() => {
  mockContext = {
    person: { userId: "user_1", email: "ada@example.test", analyticsOptIn: false },
    organizations: [
      { organizationId: "org_1", name: null, ownerEmail: "ada@example.test", role: "owner" },
    ],
    activeOrganizationId: "org_1",
    memberships: [],
    incomingInvites: [],
    flags: {},
  };
  mockProject = {
    id: "project_7bQx2mNv9BcK4dLp",
    name: "My Project",
    role: Role.Owner,
  };
});

describe("SettingsPage", () => {
  describe("project mode", () => {
    it("renders the project settings and rename form", async () => {
      render(
        await SettingsPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByTestId("page-header")).toHaveTextContent("Settings");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("My Project");
      expect(screen.getByTestId("rename-form")).toHaveTextContent("editable");
    });

    it("offers the owner a danger zone for this project, named", async () => {
      render(
        await SettingsPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
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
      render(
        await SettingsPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
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
      render(
        await SettingsPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.queryByTestId("delete-form")).toBeNull();
      expect(screen.queryByText(/danger zone/i)).toBeNull();
      expect(screen.getByTestId("rename-form")).toHaveTextContent("readonly");
    });

    it("shows outage when context is unavailable", async () => {
      mockContext = null;
      render(
        await SettingsPage({
          params: Promise.resolve({ projectId: "project_7bQx2mNv9BcK4dLp" }),
        }),
      );
      expect(screen.getByTestId("outage")).toBeInTheDocument();
    });
  });
});
