"use client";

import { usePathname } from "next/navigation";

import { useRecordConsolePath } from "@/lib/use-console-trail";

// Mounted once by the console layout, beside the other components that draw
// nothing and only listen. Recording is a property of the console, not of any
// one surface in it: the rail and the selector both READ the trail, and a
// reader that also owned the write would make "where you have been" depend on
// which surface happened to be mounted. `use-console-trail.ts` has the store.
export function ConsoleTrailRecorder() {
  useRecordConsolePath(usePathname());
  return null;
}
