// Rail rows a console built on this one adds to a group: empty here, and the
// file such a console lays its own copy over (see `./ui`). The core's rows are
// `lib/console-nav.ts`'s; these join them, so the core never names a page it
// does not have.
import type { LucideIcon } from "lucide-react";

export interface ExtraNavItem {
  /** The group the row joins. */
  group: "Project" | "Organization" | "Account";
  title: string;
  /** Under the group's root, as the core's rows are: `/reports`. */
  path: string;
  icon: LucideIcon;
  /** The row it goes above, by title; last in the group when absent. */
  before?: string;
}

export const EXTRA_NAV_ITEMS: readonly ExtraNavItem[] = [];
