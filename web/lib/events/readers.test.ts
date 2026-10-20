import { describe, expect, it } from "vitest";

import type { Membership, OrganizationEntry } from "@/lib/server/entities/organization";

import { readingKey, readsProject } from "./readers";

function access(role: OrganizationEntry["role"], seats: string[] = []) {
  return {
    organizations: [{ organizationId: "org_1", slug: "acme", name: "Acme", role }],
    memberships: seats.map(
      (projectId): Membership => ({ projectId, organizationId: "org_1", role: null }),
    ),
  };
}

describe("who reads a project", () => {
  it("is every owner and admin of its organization, seat or not", () => {
    expect(readsProject(access("owner"), "org_1", "project_1")).toBe(true);
    expect(readsProject(access("admin"), "org_1", "project_1")).toBe(true);
  });

  it("is a member only where they hold a seat", () => {
    expect(readsProject(access("member", ["project_1"]), "org_1", "project_1")).toBe(true);
    expect(readsProject(access("member", ["project_1"]), "org_1", "project_2")).toBe(false);
  });

  it("is nobody from outside the organization, whatever they own elsewhere", () => {
    expect(readsProject(access("owner"), "org_2", "project_9")).toBe(false);
  });
});

describe("the key of what a person reads", () => {
  it("moves with a role or a seat, never with the order /me lists them in", () => {
    const seated = readingKey(access("member", ["project_1", "project_2"]));
    expect(readingKey(access("member", ["project_2", "project_1"]))).toBe(seated);
    expect(readingKey(access("member", ["project_1"]))).not.toBe(seated);
    expect(readingKey(access("admin", ["project_1", "project_2"]))).not.toBe(seated);
    expect(readingKey(null)).toBe("");
  });
});
