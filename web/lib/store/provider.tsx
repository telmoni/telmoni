"use client";

import { useEffect, useState, type ReactNode } from "react";
import { StoreContext, createStore } from "./index";
import type { StoreInitial } from "./index";

interface Props extends StoreInitial {
  children: ReactNode;
}

export function StoreProvider({
  children,
  user,
  organizations,
  incomingInvites,
  projectOffers,
  activeOrganizationId,
  flags,
  roles,
  projects,
  projectsElsewhere,
}: Props) {
  const [store] = useState(() =>
    createStore({
      user,
      organizations,
      incomingInvites,
      projectOffers,
      activeOrganizationId,
      flags,
      roles,
      projects,
      projectsElsewhere,
    }),
  );

  // ⚠ **EVERY prop app/(app)/layout.tsx hands over is written back here, and
  // the list being partial was a bug.** `createStore` runs once per MOUNT, and
  // every navigation inside `app/(app)` is a client-side one — the layout
  // re-renders on the server and this component does not remount. Anything
  // left out is one server render's answer drawn beside another's.
  //
  // `activeOrganizationId` was the one that showed. Switching organizations
  // reseeded `projects` from the organization you moved into while the id stayed
  // as it was at mount, and the resource selector reads the two together: it
  // drew your member seat on somebody else's project underneath the name and
  // Owner badge of the organization you had left.
  //
  // The props are new objects on every payload, so the dependencies below fire
  // whenever the server answers — including the `router.refresh()` the
  // realtime listener makes on every invite event. Whether that payload SAYS
  // anything new is `setSeed`'s question, not this component's: it compares
  // against the last seed and writes only what moved.
  useEffect(() => {
    store.getState().setSeed({
      user,
      organizations,
      incomingInvites,
      projectOffers,
      activeOrganizationId,
      flags,
      roles,
      projects,
      projectsElsewhere,
    });
  }, [
    store,
    projects,
    projectsElsewhere,
    incomingInvites,
    projectOffers,
    roles,
    organizations,
    flags,
    user,
    activeOrganizationId,
  ]);

  return (
    <StoreContext.Provider value={store}>
      {children}
    </StoreContext.Provider>
  );
}
