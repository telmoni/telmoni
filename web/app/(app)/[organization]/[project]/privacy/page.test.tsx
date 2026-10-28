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
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchContentMode = vi.fn<(projectId: string) => Promise<unknown>>(
  async () => mockContentMode,
);
const fetchFieldInventory = vi.fn<(projectId: string) => Promise<unknown>>(
  async () => mockInventory,
);

vi.mock("@/lib/server/data", () => ({
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchContentMode: (projectId: string) => fetchContentMode(projectId),
  fetchFieldInventory: (projectId: string) => fetchFieldInventory(projectId),
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
        needs: "on, or sealed from our SDK",
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
    expect(screen.getByText("on, or sealed from our SDK")).toBeInTheDocument();
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

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await PrivacyPage({ params: PARAMS, searchParams: Promise.resolve({}) }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
