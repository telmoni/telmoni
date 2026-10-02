// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      <span>{title}</span>
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

vi.mock("./_manage", () => ({
  KeysActions: ({ projectId }: { projectId: string }) => (
    <div data-testid="keys-actions" data-project={projectId} />
  ),
}));

vi.mock("next/navigation", () => ({
  useRouter: () => ({ refresh: () => {} }),
}));


import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockTokens: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchTokens = vi.fn<(projectId: string) => Promise<unknown>>(async () => mockTokens);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchTokens: (projectId: string) => fetchTokens(projectId),
}));

import ApiKeysPage from "./page";

// The path spells the project with slugs; the page and its actions use its id.
const PARAMS = Promise.resolve({ organization: "acme", project: "my-project" });

beforeEach(() => {
  vi.clearAllMocks();
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
  mockTokens = { kind: "ok", tokens: [] };
});

describe("ApiKeysPage", () => {
  it("renders the API keys page for a project", async () => {
    render(await ApiKeysPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("API keys");
    expect(screen.getByTestId("keys-actions")).toBeInTheDocument();
  });

  it("finds the project by the path's slug, and lists and mints by its id", async () => {
    render(await ApiKeysPage({ params: PARAMS }));
    expect(fetchProjectBySlug).toHaveBeenCalledWith("my-project");
    expect(fetchTokens).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp");
    expect(screen.getByTestId("keys-actions")).toHaveAttribute(
      "data-project",
      "project_7bQx2mNv9BcK4dLp",
    );
  });

  it("sends a refused role back to the project's overview", async () => {
    mockTokens = { kind: "forbidden" };
    render(await ApiKeysPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/my-project",
    );
  });

  it("enables mint key and row actions for admin role", async () => {
    mockProject = {
      id: "project_7bQx2mNv9BcK4dLp",
      name: "My Project",
      role: Role.Admin,
    };
    mockTokens = {
      kind: "ok",
      tokens: [
        {
          id: "tok_1",
          name: "Admin key",
          expires_at: null,
          last_used_at: null,
          created_at: "2026-09-01T00:00:00Z",
        },
      ],
    };

    render(await ApiKeysPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("API keys");
    expect(screen.getByTestId("keys-actions")).toBeInTheDocument();
    expect(screen.getByText("Admin key")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Rotate/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Revoke/i })).toBeInTheDocument();
  });

  it("hides mint key and row actions for member role", async () => {
    mockProject = {
      id: "project_7bQx2mNv9BcK4dLp",
      name: "My Project",
      role: Role.Member,
    };
    mockTokens = {
      kind: "ok",
      tokens: [
        {
          id: "tok_1",
          name: "Read only key",
          expires_at: null,
          last_used_at: null,
          created_at: "2026-09-01T00:00:00Z",
        },
      ],
    };

    render(await ApiKeysPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("API keys");
    expect(screen.queryByTestId("keys-actions")).toBeNull();
    expect(screen.getByText("Read only key")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Rotate/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Revoke/i })).toBeNull();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await ApiKeysPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
