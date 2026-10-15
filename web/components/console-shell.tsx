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

type ConsoleShellValue = {
  /** The content area — under the header, beside the rail — for a panel of
   *  the page's own to stand in over the page and cover neither: the trace
   *  list's peek (`components/traces/peek.tsx`). It does not scroll with the
   *  page, so the panel holds still as the list moves under it. `null` until
   *  it is drawn. */
  element: HTMLElement | null;
  /** Says a panel covers the page whole, which takes the page out of reach of
   *  the keyboard and of a screen reader (`inert`) until it says otherwise.
   *  Set on the element at once rather than drawn, so a panel shutting can
   *  hand the focus back to the page in the same moment. */
  setCovered: (covered: boolean) => void;
};

const ConsoleShellContext = createContext<ConsoleShellValue>({
  element: null,
  setCovered: () => {},
});

export function useConsoleShell(): ConsoleShellValue {
  return useContext(ConsoleShellContext);
}

export function ConsoleShell({ children }: { children: ReactNode }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const [shell, setShell] = useState<HTMLDivElement | null>(null);
  const holdShell = useCallback((element: HTMLDivElement | null) => {
    shellRef.current = element;
    setShell(element);
  }, []);
  const setCovered = useCallback((covered: boolean) => {
    if (mainRef.current) mainRef.current.inert = covered;
  }, []);
  const value = useMemo(() => ({ element: shell, setCovered }), [shell, setCovered]);
  const pathname = usePathname();

  // A new page opens at its top. Next's own scrolling on a navigation is the
  // document's, and keeps the old offset while the page's top is in view
  // (`scroll` in the Link docs); the console scrolls in this viewport instead,
  // where a new page would otherwise open where the last one was left.
  useEffect(() => {
    rootRef.current
      ?.querySelector('[data-slot="scroll-area-viewport"]')
      ?.scrollTo({ top: 0 });
  }, [pathname]);

  const prevPath = useRef(pathname);
  useEffect(() => {
    if (prevPath.current === pathname) return;
    prevPath.current = pathname;
    const raf = requestAnimationFrame(() => {
      const root = shellRef.current;
      const target =
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
        <AgentPanel />
      </div>
    </ConsoleShellContext.Provider>
  );
}
