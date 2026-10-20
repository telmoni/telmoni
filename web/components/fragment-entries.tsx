"use client";

import { Suspense, useEffect, useRef } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

/** Hands the router a traversal onto an entry a fragment made — a hash typed into the address bar,
 *  a docs heading's anchor — which has no state and which Next's router passes over, so the page
 *  before it would stay on show under its address: a peek left open over the list, the console
 *  over the docs. */
export function FragmentEntries() {
  return (
    <Suspense fallback={null}>
      <Follower />
    </Suspense>
  );
}

function Follower() {
  const router = useRouter();
  const pathname = usePathname();
  const search = useSearchParams().toString();
  const shown = useRef({ pathname, search });
  useEffect(() => {
    shown.current = { pathname, search };
  }, [pathname, search]);
  useEffect(() => {
    const landed = (event: PopStateEvent) => {
      if (event.state !== null) return;
      const { pathname, search, hash } = window.location;
      const here = `${pathname}${search}${hash}`;
      // The page on show: Next's own `replaceState` follows it, where `replace` reads it again.
      if (pathname === shown.current.pathname && new URLSearchParams(search).toString() === shown.current.search) {
        window.history.replaceState({}, "", here);
      } else {
        router.replace(here, { scroll: false });
      }
    };
    window.addEventListener("popstate", landed);
    return () => window.removeEventListener("popstate", landed);
  }, [router]);
  return null;
}
