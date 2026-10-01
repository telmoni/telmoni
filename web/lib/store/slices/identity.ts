import type { StateCreator } from "zustand";
import { compareProjects } from "@/lib/projects";
import type { AppStore, IdentitySlice, Seed, StoreInitial } from "../types";

const NONE: readonly never[] = [];

/// ⚠ **Where the fallbacks live, and the only place they may.** The layout
/// hands over the organization auth resolved; `null` means `/me` answered
/// nothing, and it stays `null` rather than resolving against anything in the
/// store — the fallback would answer with the organization you just left, and
/// the console would go on naming it.
export function normalizeSeed(initial: StoreInitial): Seed {
  return {
    user: initial.user,
    organizations: initial.organizations ?? NONE,
    incomingInvites: initial.incomingInvites ?? NONE,
    projectOffers: initial.projectOffers ?? NONE,
    activeOrganizationId: initial.activeOrganizationId ?? null,
    flags: initial.flags,
    roles: initial.roles,
    projects: initial.projects,
    projectsElsewhere: initial.projectsElsewhere ?? NONE,
  };
}

/// Value equality over what the server sends: JSON, so primitives, arrays and
/// plain objects. Recursive rather than one level deep — every entity in the
/// seed is flat today, and a nested one added later would otherwise compare
/// equal while holding different data, which reads as the console ignoring a
/// change it was told about.
export function same(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) {
    return false;
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, i) => same(item, b[i]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  return keys.every(
    (k) => Object.hasOwn(right, k) && same(left[k], right[k]),
  );
}

export const createIdentitySlice = (
  initial: StoreInitial,
): StateCreator<AppStore, [], [], IdentitySlice> =>
  (set) => {
    const seed = normalizeSeed(initial);
    return {
      ...seed,
      serverSeed: seed,

      renameProject: (id, name) =>
        set((s) => ({
          projects: s.projects.map((p) => (p.id === id ? { ...p, name } : p)),
        })),

      addProject: (project) =>
        set((s) => {
          if (s.projects.some((p) => p.id === project.id)) return s;
          return {
            projects: [...s.projects, project].sort(compareProjects),
          };
        }),

      addIncomingInvite: (invite) =>
        set((s) => {
          if (s.incomingInvites.some((i) => i.id === invite.id)) return s;
          return { incomingInvites: [invite, ...s.incomingInvites] };
        }),

      removeIncomingInvite: (id) =>
        set((s) => ({
          incomingInvites: s.incomingInvites.filter((i) => i.id !== id),
        })),

      // ⚠ **Compared against the last SEED, never against the live state.**
      // A local edit — an optimistic rename, an invite dismissed before the
      // server hears about it — moves the state and not the seed. Comparing
      // with the state would read that edit as a difference and overwrite it
      // with the older value the server is still sending, so the rename would
      // flicker back on the next unrelated refresh. The realtime listener
      // refreshes on every invite event, so "unrelated" is the common case.
      //
      // Field by field, so a payload that moves one thing re-renders only the
      // consumers of that thing — and leaves an optimistic edit to a DIFFERENT
      // field standing.
      setSeed: (next) =>
        set((s) => {
          const seed = normalizeSeed(next);
          const was = s.serverSeed;
          if (same(was, seed)) return s;

          const patch: Partial<Seed> & Pick<IdentitySlice, "serverSeed"> = {
            serverSeed: seed,
          };
          if (!same(was.user, seed.user)) patch.user = seed.user;
          if (!same(was.organizations, seed.organizations)) {
            patch.organizations = seed.organizations;
          }
          if (!same(was.incomingInvites, seed.incomingInvites)) {
            patch.incomingInvites = seed.incomingInvites;
          }
          if (!same(was.projectOffers, seed.projectOffers)) {
            patch.projectOffers = seed.projectOffers;
          }
          if (was.activeOrganizationId !== seed.activeOrganizationId) {
            patch.activeOrganizationId = seed.activeOrganizationId;
          }
          if (!same(was.flags, seed.flags)) patch.flags = seed.flags;
          if (!same(was.roles, seed.roles)) patch.roles = seed.roles;
          if (!same(was.projects, seed.projects)) patch.projects = seed.projects;
          if (!same(was.projectsElsewhere, seed.projectsElsewhere)) {
            patch.projectsElsewhere = seed.projectsElsewhere;
          }
          return patch;
        }),
    };
  };
