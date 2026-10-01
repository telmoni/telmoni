import type { ReactNode } from "react";

/**
 * A banner a console built on this one draws above every console page, under
 * the platform announcement: nothing here, and the file such a console lays
 * its own copy over (see `lib/extension/ui.ts`). Its copy may be an async
 * server component that reads what it needs itself; the layout passes nothing.
 */
export function ExtensionBanner(): ReactNode {
  return null;
}
