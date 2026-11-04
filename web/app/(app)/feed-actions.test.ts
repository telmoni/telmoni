import { beforeEach, describe, expect, it, vi } from "vitest";

const feed = { items: [], unread: 0 };
const fetchNotifications = vi.hoisted(() => vi.fn(async () => feed));
const fetchProjectNotifications = vi.hoisted(() => vi.fn<(projectId: string) => Promise<typeof feed>>(async () => feed));
vi.mock("@/lib/server/entities/notification-feed", () => ({ fetchNotifications, fetchProjectNotifications }));
let role = "owner";
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: async () => ({ userId: "u", organizationId: "org_acme", role, accessToken: "t" }),
}));

import { readFeedAction } from "./feed-actions";

beforeEach(() => {
  vi.clearAllMocks();
  role = "owner";
});

describe("readFeedAction", () => {
  it("reads a project's feed by its id", async () => {
    expect(await readFeedAction({ kind: "project", projectId: "project_web" })).toBe(feed);
    expect(fetchProjectNotifications).toHaveBeenCalledWith("project_web");
  });

  it("reads the organization's own for its owners and admins, in the organization the session stands in", async () => {
    expect(await readFeedAction({ kind: "organization", organizationId: "org_acme" })).toBe(feed);
    expect(await readFeedAction({ kind: "organization", organizationId: "org_other" })).toBeNull();
    role = "member";
    expect(await readFeedAction({ kind: "organization", organizationId: "org_acme" })).toBeNull();
    expect(fetchNotifications).toHaveBeenCalledOnce();
  });

  it("reads nothing for a scope it cannot read", async () => {
    for (const asked of [null, "project", { kind: "project" }, { kind: "project", projectId: "" }, { kind: "user" }]) {
      expect(await readFeedAction(asked)).toBeNull();
    }
    expect(fetchProjectNotifications).not.toHaveBeenCalled();
  });
});
