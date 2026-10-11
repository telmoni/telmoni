// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/components/page-header", () => ({
  PageHeader: ({ title, count }: { title: string; count?: number }) => (
    <div data-testid="page-header">
      {title}
      {count !== undefined && <span data-testid="count">{count}</span>}
    </div>
  ),
}));

vi.mock("@/components/service-unavailable", () => ({
  ServiceUnavailable: () => <div data-testid="outage">unavailable</div>,
}));

import { Role } from "@/lib/types/enums";

let mockContext: unknown;
let mockProject: unknown;
let mockConversations: unknown;
const fetchProjectBySlug = vi.fn<(slug: string) => Promise<unknown>>(async () => mockProject);
const fetchConversations = vi.fn<(projectId: string, query: unknown) => Promise<unknown>>(
  async () => mockConversations,
);

vi.mock("@/lib/server/data", () => ({
  activeOrganization: () => null,
  getServerContext: async () => mockContext,
  fetchProjectBySlug: (slug: string) => fetchProjectBySlug(slug),
  fetchConversations: (projectId: string, query: unknown) => fetchConversations(projectId, query),
}));

import ConversationsPage from "./page";

const PARAMS = Promise.resolve({ organization: "acme", project: "web" });
const HASHED = "9f86d081884c7d659a2feaa0c55ad015";

const CONVERSATION_A = {
  conversation_id: "conv-1",
  user_id: HASHED,
  traces: 3,
  errors: 1,
  steps: 9,
  input_tokens: 1200,
  output_tokens: 300,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:04:02Z",
};
// An id of the application's choosing, with a slash and a space, and no user.
const CONVERSATION_B = {
  conversation_id: "chat/42 b",
  user_id: "",
  traces: 1,
  errors: 0,
  steps: 2,
  input_tokens: 10,
  output_tokens: 5,
  first_seen: "2026-10-10T03:00:00Z",
  last_seen: "2026-10-10T03:00:01Z",
};

function list(over: Record<string, unknown> = {}) {
  return {
    kind: "ok",
    list: {
      conversations: [CONVERSATION_A, CONVERSATION_B],
      days: 7,
      limit: 100,
      more: false,
      sort: "last_seen",
      dir: "desc",
      ...over,
    },
  };
}

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
  mockProject = { id: "project_7bQx2mNv9BcK4dLp", slug: "web", name: "Web", role: Role.Member };
  mockConversations = list();
});

describe("ConversationsPage", () => {
  it("lists each conversation with its user and sums, the conversation opening its page and the user their traces", async () => {
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.getByTestId("page-header")).toHaveTextContent("Conversations");
    expect(screen.getByTestId("count")).toHaveTextContent("2");
    expect(fetchConversations).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {});

    expect(screen.getByRole("link", { name: "conv-1" })).toHaveAttribute(
      "href",
      "/acme/web/conversations/conv-1",
    );
    expect(screen.getByRole("link", { name: "chat/42 b" })).toHaveAttribute(
      "href",
      "/acme/web/conversations/chat%2F42%20b",
    );
    expect(screen.getByRole("link", { name: HASHED })).toHaveAttribute(
      "href",
      `/acme/web/traces?user=${HASHED}`,
    );
    // A conversation whose runs named no user shows a dash, not a link.
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.getByText("1,500")).toHaveAttribute("title", "1,200 in, 300 out");

    // Last heard from first by default; a click turns it round, and another column starts its own way.
    expect(screen.getByRole("columnheader", { name: /Last seen/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    expect(screen.getByRole("link", { name: /Last seen/ })).toHaveAttribute(
      "href",
      "/acme/web/conversations?sort=last_seen&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Conversation" })).toHaveAttribute(
      "href",
      "/acme/web/conversations?sort=conversation&dir=asc",
    );
    expect(screen.getByRole("link", { name: "Errors" })).toHaveAttribute(
      "href",
      "/acme/web/conversations?sort=errors&dir=desc",
    );
  });

  it("reads the URL's sort and window, hands them to the server, and carries them in the form", async () => {
    render(
      await ConversationsPage({
        params: PARAMS,
        searchParams: Promise.resolve({ sort: "user", dir: "desc", days: "90", limit: "0" }),
      }),
    );
    expect(fetchConversations).toHaveBeenCalledWith("project_7bQx2mNv9BcK4dLp", {
      sort: "user",
      dir: "desc",
      days: 90,
    });
    expect(screen.getByRole("columnheader", { name: /User/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    const form = screen.getByRole("form", { name: "Choose the window" });
    expect(form).toHaveAttribute("action", "/acme/web/conversations");
    expect(screen.getByRole("combobox", { name: /Window/ })).toHaveValue("90");
    expect(form.querySelector('input[name="sort"]')).toHaveValue("user");
    expect(form.querySelector('input[name="dir"]')).toHaveValue("desc");
  });

  it("says when the window holds more, and offers the larger page", async () => {
    mockConversations = list({ conversations: [CONVERSATION_A], more: true });
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.queryByTestId("count")).toBeNull();
    expect(screen.getByRole("link", { name: "Show up to 500" })).toHaveAttribute(
      "href",
      "/acme/web/conversations?limit=500",
    );
  });

  it("tells a project that names no conversation which attribute to set", async () => {
    mockConversations = list({ conversations: [] });
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.getByText("Nothing to show yet.")).toBeInTheDocument();
    expect(screen.getByText("gen_ai.conversation.id")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "SDKs" })).toHaveAttribute("href", "/docs/api/sdks");
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("sends a refused role back to the project's overview, and says so on an outage", async () => {
    mockConversations = { kind: "forbidden" };
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.getByRole("link", { name: "Back to overview" })).toHaveAttribute(
      "href",
      "/acme/web",
    );

    mockConversations = { kind: "unavailable" };
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });

  it("renders ServiceUnavailable when context fails", async () => {
    mockContext = null;
    render(await ConversationsPage({ params: PARAMS }));
    expect(screen.getByTestId("outage")).toBeInTheDocument();
  });
});
