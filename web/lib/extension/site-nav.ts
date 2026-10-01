// Primary navigation links a console built on this one adds to the splash header:
// empty here, and the file such a console lays its own copy over. The core's
// links are `components/site-nav.ts`'s; these join them, so the core never
// names a page it does not have.
export interface ExtraPrimaryNavItem {
  label: string;
  href: string;
  newTab?: boolean;
  /** The item it goes before, by label; last when absent. */
  before?: string;
}

export const EXTRA_PRIMARY_NAV: readonly ExtraPrimaryNavItem[] = [];
