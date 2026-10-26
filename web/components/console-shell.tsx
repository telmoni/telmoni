"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { usePathname } from "next/navigation";

import { AgentPanel } from "@/components/agent-panel";
import { ScrollArea } from "@/components/ui/scroll-area";
import { setPageCovered } from "@/lib/page-cover";

type ConsoleShellValue = {
  /** The content area — under the header, beside the rail — for a side panel
   *  to stand in over the page and cover neither (`side-panel.tsx`). It does
   *  not scroll with the page, so the panel holds still as the page moves
   *  under it. `null` until it is drawn. */
  element: HTMLElement | null;
  /** Takes the page out of reach of the keyboard and of a screen reader
   *  (`inert`) while a panel covers it whole, until the release it answers is
   *  called: two may cover it at once, a new project over a trace. Set on the
   *  element at once rather than drawn, so a panel shutting can hand the
   *  focus back to the page in the same moment. */
  cover: () => () => void;
};

const ConsoleShellContext = createContext<ConsoleShellValue>({
  element: null,
  cover: () => () => {},
});

export function useConsoleShell(): ConsoleShellValue {
  return useContext(ConsoleShellContext);
}

export function ConsoleShell({
  panels,
  children,
}: {
  /** The panels the console opens over any page (`create-panels.tsx`). */
  panels?: ReactNode;
  children: ReactNode;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const [shell, setShell] = useState<HTMLDivElement | null>(null);
  const holdShell = useCallback((element: HTMLDivElement | null) => {
    shellRef.current = element;
    setShell(element);
  }, []);
  const covers = useRef(new Set<symbol>());
  const cover = useCallback(() => {
    const mine = Symbol();
    covers.current.add(mine);
    if (mainRef.current) mainRef.current.inert = true;
    setPageCovered(true);
    return () => {
      covers.current.delete(mine);
      const still = covers.current.size > 0;
      if (mainRef.current) mainRef.current.inert = still;
      setPageCovered(still);
    };
  }, []);
  const value = useMemo(() => ({ element: shell, cover }), [shell, cover]);
  const pathname = usePathname();

  // A new page opens at its top, or at the element its address's fragment
  // names, as the incident a trace's page links. Next's own scrolling on a
  // navigation is the document's, and keeps the old offset while the page's
  // top is in view (`scroll` in the Link docs); the console scrolls in this
  // viewport instead, where a new page would otherwise open where the last
  // one was left.
  useEffect(() => {
    const viewport = rootRef.current?.querySelector('[data-slot="scroll-area-viewport"]');
    const named = window.location.hash.slice(1);
    const target = named ? document.getElementById(named) : null;
    if (target && viewport?.contains(target)) target.scrollIntoView({ block: "start" });
    else viewport?.scrollTo({ top: 0 });
  }, [pathname]);

  // The focus goes to the page's title, or to the element the fragment names
  // where that takes it, so a screen reader starts at the incident followed.
  const prevPath = useRef(pathname);
  useEffect(() => {
    if (prevPath.current === pathname) return;
    prevPath.current = pathname;
    const raf = requestAnimationFrame(() => {
      const root = shellRef.current;
      const named = window.location.hash.slice(1);
      const fragment = named ? document.getElementById(named) : null;
      const target =
        (fragment?.hasAttribute("tabindex") && root?.contains(fragment) ? fragment : null) ??
        root?.querySelector<HTMLElement>('h1[tabindex="-1"]') ??
        root?.querySelector<HTMLElement>("#console-main");
      target?.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(raf);
  }, [pathname]);

  useEffect(() => {
    document.body.classList.add("theme-console");
    return () => document.body.classList.remove("theme-console");
  }, []);

  return (
    <ConsoleShellContext.Provider value={value}>
      <div
        ref={holdShell}
        data-slot="console-shell"
        // The page stands on the same background as the header and the rail,
        // one colour with rules between the regions, and nothing stands between
        // it and the rail's rule but `<main>`'s own padding: 14px all round, the
        // rail's inset, which the header keeps too. The agent's window floats
        // over all of it (`components/agent/agent-window.tsx`); it comes after
        // the page so that it is reached after the page from the keyboard, and
        // drawn over the header.
        className="relative flex min-h-0 min-w-0 flex-1"
      >
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          {/* Radix wraps the content in `display: table`, which never narrows
              below its widest child; as a block the page narrows with the
              window. */}
          <ScrollArea
            ref={rootRef}
            type="scroll"
            className="min-h-0 min-w-0 flex-1 **:data-[slot=scroll-area-thumb]:bg-muted-foreground/30 [&_[data-slot=scroll-area-viewport]>div]:block!"
          >
            <div className="flex w-full min-w-0 flex-col">
              <main
                ref={mainRef}
                id="console-main"
                tabIndex={-1}
                className="flex w-full min-w-0 flex-col gap-3 p-3.5 outline-none"
              >
                {children}
              </main>
            </div>
          </ScrollArea>
        </div>
        {panels}
        <AgentPanel />
      </div>
    </ConsoleShellContext.Provider>
  );
}
