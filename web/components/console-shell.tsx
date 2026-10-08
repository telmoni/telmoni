"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { usePathname } from "next/navigation";

import { AgentPanel } from "@/components/agent-panel";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PRODUCT_NAME } from "@/lib/site";

export function ConsoleShell({ children }: { children: ReactNode }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const pathname = usePathname();

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

  useEffect(() => {
    let lastTitle = document.title || PRODUCT_NAME;
    const observer = new MutationObserver(() => {
      if (document.title) {
        lastTitle = document.title;
      } else if (lastTitle) {
        document.title = lastTitle;
      }
    });
    observer.observe(document.head, {
      childList: true,
      subtree: true,
      characterData: true,
    });
    return () => observer.disconnect();
  }, []);

  return (
    <div
      ref={shellRef}
      data-slot="console-shell"
      // The page and the agent panel stand on the same background as the
      // header and the rail, one colour with rules between the regions, and
      // nothing stands between them and the rail's rule but `<main>`'s own
      // padding: 14px all round, the rail's inset, which the header keeps too.
      className="relative flex min-h-0 min-w-0 flex-1"
    >
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        {/* Radix wraps the content in `display: table`, which never narrows
            below its widest child; as a block the page can shrink beside the
            agent panel. */}
        <ScrollArea
          ref={rootRef}
          type="scroll"
          className="min-h-0 min-w-0 flex-1 **:data-[slot=scroll-area-thumb]:bg-muted-foreground/30 [&_[data-slot=scroll-area-viewport]>div]:block!"
        >
          <div className="flex w-full min-w-0 flex-col">
            <main
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
  );
}
