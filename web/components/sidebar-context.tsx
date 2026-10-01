"use client";

import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { NAV_COLLAPSED_COOKIE } from "@/lib/console-nav";

const useIsomorphicLayoutEffect =
  typeof window === "undefined" ? useEffect : useLayoutEffect;

interface SidebarContextValue {
  collapsed: boolean;
  toggle: () => void;
  closeOnMobile: () => void;
  animating: boolean;
}

export const MOBILE_BREAKPOINT_PX = 768;

export const DRAWER_SLIDE_MS = 200;

// Something the drawer must yield to, because it opened on top of the drawer.
// One list, two readers — Escape and the outside press below dismiss for the
// same reason and must agree about what counts as "something else is in front".
const OVERLAY_SELECTOR = '[role="dialog"], [role="alertdialog"], [role="menu"]';

function useIsMobile(): boolean | null {
  const [isMobile, setIsMobile] = useState<boolean | null>(null);
  useIsomorphicLayoutEffect(() => {
    if (typeof window.matchMedia !== "function") {
      setIsMobile(false);
      return;
    }
    const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT_PX - 1}px)`);
    const onChange = () => setIsMobile(mql.matches);
    onChange();
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return isMobile;
}

const SidebarContext = createContext<SidebarContextValue | null>(null);

function persistCollapsed(collapsed: boolean) {
  document.cookie = `${NAV_COLLAPSED_COOKIE}=${collapsed ? "1" : "0"}; path=/; max-age=31536000; samesite=lax`;
}

export function SidebarProvider({
  initialCollapsed = false,
  children,
}: {
  initialCollapsed?: boolean;
  children: ReactNode;
}) {
  const isMobile = useIsMobile();

  const [open, setOpen] = useState(!initialCollapsed);

  const [animating, setAnimating] = useState(false);

  // ⚠ **Held and cleared, because this timer used to outlive the component.**
  // It fired `setAnimating` after unmount — invisible in the browser, where the
  // provider lives as long as the page, and a hard failure under a test runner
  // that treats a post-teardown state update as an unhandled error. Clearing a
  // pending one before arming the next also fixes a real case: two toggles
  // inside 200ms left the first timer to end the second one's animation early.
  const settle = useRef<number | null>(null);
  const move = (next: boolean) => {
    setAnimating(true);
    setOpen(next);
    if (settle.current !== null) window.clearTimeout(settle.current);
    settle.current = window.setTimeout(() => {
      settle.current = null;
      setAnimating(false);
    }, DRAWER_SLIDE_MS);
  };
  useEffect(
    () => () => {
      if (settle.current !== null) window.clearTimeout(settle.current);
    },
    [],
  );

  const arrived = useRef(false);
  useIsomorphicLayoutEffect(() => {
    if (isMobile === null || arrived.current) return;
    arrived.current = true;
    if (isMobile) setOpen(false);
  }, [isMobile]);

  const collapsed = !open;

  useEffect(() => {
    if (!isMobile || collapsed) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (e.defaultPrevented) return;
      if (document.querySelector(OVERLAY_SELECTOR)) return;
      move(false);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [isMobile, collapsed]);

  // ⚠ **A press outside closes the drawer, and there is deliberately no scrim
  // to catch it.** The usual way to do this is a full-page overlay, and
  // `console-sidebar.test.tsx` pins that this drawer draws none — the page
  // behind stays visible and undimmed, which is the reason the rail slides
  // over it rather than covering it. So the press is caught on the document.
  //
  // `pointerdown` and not `click`: one listener covers mouse, touch and pen,
  // and it lands before focus moves, so a press aimed at something in the page
  // gets the drawer out of the way AND still does what it was aimed at.
  useEffect(() => {
    if (!isMobile || collapsed) return;
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target;
      if (!(target instanceof Element)) return;
      // Inside the drawer is not outside it. A row that navigates is the
      // rail's own business — it closes there, where the back arrow's
      // exemption lives.
      if (target.closest("#console-sidebar")) return;
      // ⚠ The toggle owns this state and it sits OUTSIDE the drawer, in the
      // header. Closing here would land first and the button's own click would
      // reopen — a control that visibly does nothing. `aria-controls` already
      // names the relationship, so nothing new has to be marked up for this.
      if (target.closest('[aria-controls="console-sidebar"]')) return;
      // The same yield Escape makes: whatever is in front gets the press.
      if (document.querySelector(OVERLAY_SELECTOR)) return;
      move(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [isMobile, collapsed]);

  const toggle = () => {
    const next = !open;
    move(next);
    if (!isMobile) persistCollapsed(!next);
  };

  const closeOnMobile = () => {
    if (isMobile) move(false);
  };

  return (
    <SidebarContext.Provider value={{ collapsed, toggle, closeOnMobile, animating }}>
      {children}
    </SidebarContext.Provider>
  );
}

// No no-op fallback: one that reports collapsed: false with a toggle that does
// nothing renders a rail nobody can close, which reads as a broken button
// rather than as the missing provider it is.
export function useSidebar(): SidebarContextValue {
  const ctx = useContext(SidebarContext);
  if (!ctx) {
    throw new Error("useSidebar requires a <SidebarProvider> ancestor");
  }
  return ctx;
}
