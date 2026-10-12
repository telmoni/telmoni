"use client";

import { useLinkStatus } from "next/link";

// What a list's link that asks its own page for itself again — another sort,
// filter or page, the same path with other search params — puts inside it.
// The route's `loading.tsx` stands in only for a new path, so nothing would
// say the list is on its way: while it is, this marks the link, and the list
// it sits in dims (`LIST_DIMS_WHILE_PENDING`), as it does for the list's own
// controls, which mark themselves the same way. Nothing to see itself, so it
// moves nothing.
export function LinkPending() {
  const { pending } = useLinkStatus();
  return <span hidden data-pending={pending ? "" : undefined} />;
}
