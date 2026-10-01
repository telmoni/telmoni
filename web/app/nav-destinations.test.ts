import { existsSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { PRIMARY_NAV, navLinks } from "@/components/site-nav";

const APP = __dirname;

// ⚠ **The header is the one place a dead link is invisible to us and obvious to
// everybody else.** A nav label whose page does not exist renders perfectly and
// answers 404, and nothing else in this tree checks it: the panels were added
// with `Resources` pointing at a page that had never been written, and the only
// thing that caught it was reading the route list by hand. This walks the nav
// instead.
function routeExists(href: string): boolean {
  const clean = href.split(/[?#]/)[0].replace(/\/+$/, "");
  const dir = clean === "" ? APP : path.join(APP, ...clean.slice(1).split("/"));
  return ["page.tsx", "page.ts", "route.ts", "route.tsx"].some((f) =>
    existsSync(path.join(dir, f)),
  );
}

const internal = navLinks().filter((l) => l.href.startsWith("/"));
const external = navLinks().filter((l) => /^https?:/.test(l.href));

describe("every destination the primary nav offers", () => {
  it.each(internal.map((l) => [l.label, l.href] as const))(
    "%s has a route behind it (%s)",
    (_label, href) => {
      expect(routeExists(href), `no page or route file for ${href}`).toBe(true);
    },
  );

  // Not style: a nav item that replaces the page you are reading with somebody
  // else's site has taken the back button with it.
  it.each(external.map((l) => [l.label, l.href] as const))(
    "%s leaves for another origin in a new tab (%s)",
    (_label, href) => {
      const link = navLinks().find((l) => l.href === href);
      expect(link?.newTab, `${href} is off-origin and would replace the page`).toBe(true);
    },
  );

  it("offers each destination once, from one place in the nav", () => {
    const hrefs = navLinks().map((l) => l.href);
    expect(new Set(hrefs).size, `duplicated: ${hrefs.join(", ")}`).toBe(hrefs.length);
  });

  // The type says so; this says so at runtime, because the type is only as
  // good as the next person's cast.
  it("gives a label either a destination or a panel, never both and never neither", () => {
    for (const item of PRIMARY_NAV) {
      expect(
        Boolean(item.href) !== Boolean(item.panel),
        `${item.label} has ${item.href ? "an href" : "no href"} and ${item.panel ? "a panel" : "no panel"}`,
      ).toBe(true);
    }
  });

  it("gives every panel at least one feature to lead with", () => {
    for (const item of PRIMARY_NAV) {
      if (!item.panel) continue;
      expect(item.panel.features.length, `${item.label}'s panel leads with nothing`).toBeGreaterThan(0);
    }
  });
});
