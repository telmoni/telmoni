"use client";

import { useLayoutEffect } from "react";

import { bootAccessibility } from "@/lib/use-accessibility";
import { bootCorners } from "@/lib/use-corners";

// The boot script's work (`app/layout.tsx`), done from the browser on a page
// it never ran on: a 404 the console answers from a layout's `notFound()` is
// Next's error shell, which the browser renders from scratch, and no inline
// script in a tree rendered that way runs. Before paint, so a sharp console
// never shows rounded first; on a page the script ran on, nothing changes.
export function SettingsBoot() {
  useLayoutEffect(() => {
    bootCorners();
    bootAccessibility();
  }, []);
  return null;
}
