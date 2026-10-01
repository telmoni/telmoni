import { describe, expect, it, vi } from "vitest";

import { createStore } from "../index";
import type { StoreInitial } from "../types";
import { same } from "./identity";

function organization(who: string): NonNullable<StoreInitial["organizations"]>[number] {
  return {
    organizationId: `org_${who}`,
    name: null,
    ownerEmail: `${who}@example.test`,
    ownerDisplayName: null,
    role: "owner",
  };
}

const project = (id: string, name: string) =>
  ({ id, name, role: "owner" }) as StoreInitial["projects"][number];

function payload(over: Partial<StoreInitial> = {}): StoreInitial {
  return {
    user: null,
    organizations: [organization("alpha")],
    activeOrganizationId: "org_alpha",
    flags: { beta_access: true },
    roles: { "t-1": "owner" } as StoreInitial["roles"],
    projects: [project("t-1", "Alpha")],
    ...over,
  };
}

function open(initial = payload()) {
  const store = createStore(initial);
  const writes = vi.fn();
  store.subscribe(writes);
  return { store, writes, seed: (next: StoreInitial) => store.getState().setSeed(next) };
}

describe("setSeed", () => {
  // ⚠ Auth resolves the active organization; `null` means `/me` answered
  // nothing, and nothing in the store or the payload may invent an id for it.
  it("keeps an absent id absent, rather than naming the organization just left", () => {
    const { store, seed } = open();
    expect(store.getState().activeOrganizationId).toBe("org_alpha");

    seed(payload({ organizations: [organization("beta")], activeOrganizationId: null }));

    expect(store.getState().activeOrganizationId).toBeNull();
  });

  it("never picks an organization out of the list for itself", () => {
    const { store } = open(payload({ activeOrganizationId: undefined }));
    expect(store.getState().activeOrganizationId).toBeNull();
    expect(store.getState().organizations).toHaveLength(1);
  });

  it("writes nothing at all when the server repeats itself", () => {
    const { writes, seed } = open();

    seed(payload());
    seed(payload());

    expect(writes, "an identical payload still notified consumers").not.toHaveBeenCalled();
  });

  // One payload moves one thing; the consumers of everything else must not be
  // told. Identity is the whole signal — `useSyncExternalStore` compares the
  // selected value with `Object.is`, so handing back an equal-but-new array
  // re-renders every list in the console.
  it("leaves the fields that did not move on their own identities", () => {
    const { store, writes, seed } = open();
    const before = store.getState();

    seed(payload({ flags: { beta_access: false } }));

    const after = store.getState();
    expect(after.flags, "the field that moved").toEqual({ beta_access: false });
    expect(after.projects, "projects was replaced by an equal array").toBe(before.projects);
    expect(after.roles, "roles was replaced by an equal object").toBe(before.roles);
    expect(writes, "one write, not one per field").toHaveBeenCalledTimes(1);
  });

  // ⚠ **A local edit outlives a payload that says nothing about it.** The
  // realtime listener calls `router.refresh()` on every invite event, so a
  // payload arrives within seconds of any optimistic write — and the rename
  // the person just made would flicker back to the old name if the comparison
  // were against the live state rather than against the last seed.
  it("keeps an optimistic rename through an unrelated payload", () => {
    const { store, seed } = open();
    store.getState().renameProject("t-1", "Renamed");

    seed(payload({ flags: { beta_access: false } }));

    expect(store.getState().projects[0]?.name).toBe("Renamed");
  });

  // The other half of the same rule: when the server does speak about a field,
  // it wins. Otherwise a local edit would pin the console to a stale value
  // that nothing could ever correct.
  it("takes the server's word when the payload moves that field", () => {
    const { store, seed } = open();
    store.getState().renameProject("t-1", "Renamed");

    seed(payload({ projects: [project("t-1", "Alpha"), project("t-2", "Beta")] }));

    expect(store.getState().projects.map((t) => t.name)).toEqual(["Alpha", "Beta"]);
  });
});

// The bell counts a project offer beside the invitations, so it is seeded
// like them: none when the payload carries none, and moved when it does.
describe("project offers", () => {
  it("are none until the payload lists one, and follow the payload", () => {
    const { store, writes, seed } = open();
    expect(store.getState().projectOffers).toEqual([]);

    const offered = {
      projectId: "project_1",
      name: "Payments",
      organizationId: "org_beta",
      organizationName: "Beta",
      ownerEmail: "beta@example.test",
      ownerDisplayName: null,
      expiresAt: "2026-10-05T00:00:00Z",
    };
    seed(payload({ projectOffers: [offered] }));
    expect(store.getState().projectOffers).toEqual([offered]);
    expect(writes).toHaveBeenCalledTimes(1);

    seed(payload({ projectOffers: [offered] }));
    expect(writes, "an unchanged offer still notified consumers").toHaveBeenCalledTimes(1);
  });
});

describe("same", () => {
  it("compares JSON by value, at every depth", () => {
    expect(same({ a: [1, { b: "x" }] }, { a: [1, { b: "x" }] })).toBe(true);
    expect(same({ a: [1, { b: "x" }] }, { a: [1, { b: "y" }] })).toBe(false);
    expect(same([{ id: 1 }], [{ id: 1 }, { id: 2 }])).toBe(false);
    expect(same({ a: 1 }, { a: 1, b: undefined })).toBe(false);
    expect(same(null, undefined)).toBe(false);
  });
});
