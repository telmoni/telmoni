"use client";

import { useTransition } from "react";
import { useRouter } from "next/navigation";

/** Moves the trace list to `href`, the scroll left where it is, as a click on
 *  a filter or a page does; `pending` while the next page is on its way. A
 *  control that shows its own choice passes `show`, its `useOptimistic`
 *  setter's call, to show the choice at once rather than when the list lands:
 *  it runs in the same transition, as an optimistic update has to. */
export function useListNavigation(): {
  go: (href: string, show?: () => void) => void;
  pending: boolean;
} {
  const router = useRouter();
  const [pending, start] = useTransition();
  return {
    go: (href, show) =>
      start(() => {
        show?.();
        router.push(href, { scroll: false });
      }),
    pending,
  };
}
