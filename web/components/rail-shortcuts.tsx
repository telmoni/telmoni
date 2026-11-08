"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

import { inFront, pressedKey, typedInto } from "@/lib/keys";
import { letterKeysOn } from "@/lib/use-accessibility";

export interface RailShortcut {
  key: string;
  href: string;
  newTab?: boolean;
}

/**
 * Editorial rail keyboard shortcuts: pressing digit keys (1..9) activates
 * the corresponding bracketed numbered nav item.
 *
 * Honors accessibility letterKeys setting, and ignores keys when typing in
 * input fields or when modals/dialogs are open.
 */
export function RailShortcuts({ shortcuts }: { shortcuts: readonly RailShortcut[] }) {
  const router = useRouter();

  useEffect(() => {
    const rail = document.querySelector<HTMLElement>("[data-public-rail]");
    const footer = rail?.querySelector("[data-rail-footer]");
    if (!rail || !footer || typeof IntersectionObserver === "undefined") return;

    // No flag on a page that cannot scroll: both blocks stay shown, as there
    // is no scrolling that could ever bring the primary links back.
    // A resize alone never sets the flag: its ratio may be a frame stale, so a
    // page that grows waits for the observer's next reading.
    let ratio = 0;
    const scrolls = () => document.documentElement.scrollHeight > window.innerHeight + 1;
    const observer = new IntersectionObserver(([entry]) => {
      ratio = entry?.intersectionRatio ?? 0;
      if (scrolls()) rail.dataset.footerVisible = String(ratio >= 0.99);
      else delete rail.dataset.footerVisible;
    }, { threshold: 0.99 });
    observer.observe(footer);
    const resize = () => {
      if (!scrolls()) delete rail.dataset.footerVisible;
    };
    const resized = new ResizeObserver(resize);
    resized.observe(document.body);
    window.addEventListener("resize", resize);
    return () => {
      observer.disconnect();
      resized.disconnect();
      window.removeEventListener("resize", resize);
      delete rail.dataset.footerVisible;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || typedInto(e) || inFront()) return;
      if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || !letterKeysOn()) return;

      const key = pressedKey(e);
      const match = shortcuts.find((s) => s.key === key);
      if (!match) return;

      e.preventDefault();
      if (match.newTab || /^https?:\/\//.test(match.href)) {
        window.open(match.href, "_blank", "noreferrer");
      } else if (match.href.startsWith("/auth/")) {
        // Route handlers require a full page load
        window.location.assign(match.href);
      } else {
        router.push(match.href);
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shortcuts, router]);

  return null;
}
