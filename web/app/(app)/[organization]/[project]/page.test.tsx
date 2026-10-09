// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, action }: { title?: string; action?: React.ReactNode }) => (
    <div data-testid="page-header">
      {title}
      {action}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

let mockProject: { id: string; slug: string; name: string; role: string } | null;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<typeof mockProject>>(
  async () => mockProject,
);

vi.mock("@/lib/server/data", () => ({
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
}));

import OverviewPage, { generateMetadata } from "./page";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const SLUG = "platform";

async function renderAt(project: string) {
  render(await OverviewPage({ params: Promise.resolve({ project }) }));
}

beforeEach(() => {
  vi.clearAllMocks();
  mockProject = { id: PROJECT, slug: SLUG, name: "Platform", role: "owner" };
});

describe("OverviewPage", () => {
  // The page is the project's name and nothing else, as the organization's is.
  it("is titled by the project, and says nothing else", async () => {
    await renderAt(SLUG);
    expect(screen.getByTestId("page-header")).toHaveTextContent("Platform");
    expect(screen.queryByText(PROJECT)).toBeNull();
    expect(screen.queryAllByRole("link")).toHaveLength(0);
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("finds the project by the path's slug", async () => {
    await renderAt(SLUG);
    expect(fetchProjectBySlug).toHaveBeenCalledWith(SLUG);
  });

  it("shows the outage card when the project cannot be read", async () => {
    mockProject = null;
    await renderAt(SLUG);
    expect(screen.getByTestId("outage")).toBeInTheDocument();
    expect(screen.queryByText("Platform")).toBeNull();
  });

  it("titles the tab by the project", async () => {
    expect(await generateMetadata({ params: Promise.resolve({ project: SLUG }) })).toEqual({
      title: "Platform",
    });
  });
});
