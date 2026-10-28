import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, identityContext, fetchProject } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  identityContext: vi.fn(),
  fetchProject: vi.fn(),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("./identity-context", () => ({
  identityContext,
  organizationHeaders: () => ({ authorization: "Bearer at_1", "x-organization-id": "org_1" }),
  projectHeaders: (_ctx: unknown, projectId: string) => ({
    authorization: "Bearer at_1",
    "x-organization-id": "org_1",
    "x-project-id": projectId,
  }),
}));
vi.mock("./projects", () => ({ fetchProject }));
import { fetchNotifications, fetchProjectNotifications } from "./notification-feed";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";
const RULE = "7a1c0e52-0000-4000-8000-000000000002";
const FEED = {
  items: [
    {
      id: "n1",
      kind: "connector_connected",
      title: "Slack connected",
      body: "This project's notices will now be posted to #agents in Acme.",
      metadata: { provider: "slack", channel: "agents" },
      read_at: null,
      created_at: "2026-10-14T03:00:00Z",
    },
  ],
  unread: 1,
};
// Only an incident's notice keeps its metadata.
const READ = { ...FEED, items: FEED.items.map((item) => ({ ...item, metadata: null })) };

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  fetchProject.mockResolvedValue({ id: PROJECT, slug: "platform", name: "Platform" });
  answer(200, FEED);
});

// The organization's feed and a project's are different sets, chosen by
// whether the request names a project: each reader asks for its own.
describe("fetchProjectNotifications", () => {
  it("reads the project's own feed, naming the project", async () => {
    expect(await fetchProjectNotifications(PROJECT)).toEqual(READ);
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://server/internal/v1/notifications/feed");
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
  });

  it("asks nothing for a project the person is not on, or without a signed-in person", async () => {
    // An empty id would name no project, and read the organization's feed.
    expect(await fetchProjectNotifications("")).toBeNull();
    expect(fetchProject).not.toHaveBeenCalled();
    fetchProject.mockResolvedValueOnce(null);
    expect(await fetchProjectNotifications(PROJECT)).toBeNull();
    identityContext.mockResolvedValueOnce(null);
    expect(await fetchProjectNotifications(PROJECT)).toBeNull();
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers nothing when the service refuses, answers what it cannot read, or is not there", async () => {
    answer(403, FEED);
    expect(await fetchProjectNotifications(PROJECT)).toBeNull();
    answer(200, { items: "none" });
    expect(await fetchProjectNotifications(PROJECT)).toBeNull();
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchProjectNotifications(PROJECT)).toBeNull();
  });

  // ⚠ An incident's notice names the incident its row links to; one whose
  // metadata names none stays on the feed, linking nowhere.
  it("keeps an incident's notice's metadata, checked, and no other notice's", async () => {
    const [notice] = FEED.items;
    answer(200, {
      items: [
        {
          ...notice,
          id: "n3",
          kind: "incident_opened",
          metadata: { incident_id: INCIDENT, rule_id: RULE, severity: "alert", reminder: false },
        },
        { ...notice, id: "n2", kind: "incident_resolved", metadata: { incident_id: "i-1", rule_id: RULE } },
        notice,
      ],
      unread: 3,
    });
    const read = await fetchProjectNotifications(PROJECT);
    expect(read?.items.map((item) => [item.id, item.metadata])).toEqual([
      ["n3", { incident_id: INCIDENT, rule_id: RULE }],
      ["n2", null],
      ["n1", null],
    ]);
  });
});

describe("fetchNotifications", () => {
  it("reads the organization's own feed, naming no project", async () => {
    expect(await fetchNotifications()).toEqual(READ);
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://server/internal/v1/notifications/feed");
    expect(init.headers).not.toHaveProperty("x-project-id");
    expect(fetchProject).not.toHaveBeenCalled();
  });
});
