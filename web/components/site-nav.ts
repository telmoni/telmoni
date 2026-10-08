import { EXTRA_PRIMARY_NAV, type ExtraPrimaryNavItem } from "@/lib/extension/site-nav";
import { DOCS_URL } from "@/lib/site";

export type NavLink = { label: string; href: string; newTab?: boolean };

/** A link drawn large at the top of a panel, with a sentence under it. */
export type NavFeature = NavLink & { detail: string };

/** A titled column of plain links, drawn under the features. */
export type NavGroup = { title: string; links: NavLink[] };

export type NavPanel = { features: NavFeature[]; groups?: NavGroup[] };

/**
 * One entry in the primary nav: EITHER a link or a panel, never both. A label
 * that opens a panel is a trigger and goes nowhere on its own, which is why
 * `href` and `panel` exclude each other in the type rather than by convention.
 */
export type NavItem =
  | { label: string; href: string; newTab?: boolean; panel?: never }
  | { label: string; panel: NavPanel; href?: never; newTab?: never };

const CORE_PRIMARY_NAV: readonly NavItem[] = [
  { label: "Product", href: "/" },
  { label: "Docs", href: DOCS_URL },
];

/** The core's primary nav with links built on this console joined in. */
export function withExtraPrimaryNav(
  core: readonly NavItem[],
  extras: readonly ExtraPrimaryNavItem[],
): NavItem[] {
  const items = [...core];
  for (const extra of extras) {
    const item: NavItem = {
      label: extra.label,
      href: extra.href,
      ...(extra.newTab ? { newTab: true } : {}),
    };
    const at = extra.before
      ? items.findIndex((i) => i.label === extra.before)
      : -1;
    if (at === -1) {
      items.push(item);
    } else {
      items.splice(at, 0, item);
    }
  }
  return items;
}

/**
 * ⚠ **Two plain links, and no panel, because nothing here has two pages to
 * sit side by side.** The company pages — about, security, the legal
 * documents — are the operator's, not the product's, and left this
 * repository; the footer draws the legal links from the deployment's
 * configuration (`lib/server/branding.ts`). A panel holding a single card is
 * a worse link than a link, so `Docs` is a link. Add a panel back with the
 * second page that earns it, and not before.
 *
 * Both the header row and the drawer in `mobile-nav.tsx` read this list, so an
 * entry added here appears in both. `app/nav-destinations.test.ts` walks it and
 * refuses an internal href with no route behind it, an off-origin href that
 * would replace the page, or a destination listed twice.
 */
export const PRIMARY_NAV: NavItem[] = withExtraPrimaryNav(
  CORE_PRIMARY_NAV,
  EXTRA_PRIMARY_NAV,
);

/** One panel's links in reading order, features first. The drawer has no room
 *  to draw a panel, so it lists these under the panel's label instead. */
export function panelLinks(panel: NavPanel): NavLink[] {
  return [
    ...panel.features,
    ...(panel.groups?.flatMap((group) => group.links) ?? []),
  ];
}

/** Every link the nav can reach, panels flattened. Read by the destination
 *  test, so it does not have to know the shape. */
export function navLinks(): NavLink[] {
  return PRIMARY_NAV.flatMap((item) =>
    item.panel
      ? panelLinks(item.panel)
      : [{ label: item.label, href: item.href, newTab: item.newTab }],
  );
}
