"use client";

import { createContext, useContext } from "react";
import { createStore as createZustandStore, useStore as useZustandStore, type StoreApi as ZustandStoreApi } from "zustand";
import { createIdentitySlice } from "./slices/identity";
import type { AppStore, StoreInitial } from "./types";

export type { StoreInitial };

type StoreApi = ZustandStoreApi<AppStore>;

export function createStore(initial: StoreInitial): StoreApi {
  return createZustandStore<AppStore>()((...a) => ({
    ...createIdentitySlice(initial)(...a),
  }));
}

export const StoreContext = createContext<StoreApi | null>(null);

// There is deliberately no fallback store. This module is "use client" but
// still evaluates on the server, so a module-scope store would be one object
// shared by every request in the node process — and addProject/renameProject hand
// out mutators bound to whatever they are given. Every consumer already sits
// under the provider in app/(app)/layout.tsx; a missing one is a wiring
// mistake, and it should say so rather than render an empty console.
function useStoreApi(): StoreApi {
  const store = useContext(StoreContext);
  if (!store) {
    throw new Error("store hooks require a <StoreProvider> ancestor");
  }
  return store;
}

function useStore<T>(selector: (state: AppStore) => T): T {
  return useZustandStore(useStoreApi(), selector);
}

export const useUser = () => useStore((s) => s.user);
export const useOrganizations = () => useStore((s) => s.organizations);
export const useIncomingInvites = () => useStore((s) => s.incomingInvites);
export const useProjectOffers = () => useStore((s) => s.projectOffers);
export const useActiveOrganizationId = () => useStore((s) => s.activeOrganizationId);
/** The entry for the organization the console stands in. */
export const useActiveOrganization = () =>
  useStore(
    (s) => s.organizations.find((o) => o.organizationId === s.activeOrganizationId) ?? null,
  );
export const useFlags = () => useStore((s) => s.flags);
export const useRoles = () => useStore((s) => s.roles);
export const useProjects = () => useStore((s) => s.projects);
export const useProjectsElsewhere = () => useStore((s) => s.projectsElsewhere);

export const useRenameProject = () => useStore((s) => s.renameProject);

export const useAddProject = () => useStore((s) => s.addProject);
// ⚠ **Selected, not read off `getState()` during render.** An action's
// identity never moves — `set` merges a partial over the same function
// references — so both forms hand back the same callback and neither
// re-renders. The difference is that a selector goes through
// `useSyncExternalStore`, which is what makes a render-time read of a mutable
// external store safe under concurrent rendering; `getState()` in a component
// body is the exact pattern that hook exists to replace. Two of these read one
// way and eleven read the other, which is also how a rule stops being one.
export const useAddIncomingInvite = () => useStore((s) => s.addIncomingInvite);
export const useRemoveIncomingInvite = () =>
  useStore((s) => s.removeIncomingInvite);

