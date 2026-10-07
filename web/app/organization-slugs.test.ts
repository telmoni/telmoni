import { readdirSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";
// @ts-expect-error Next ships no typings for its bundled copy
import { pathToRegexp } from "next/dist/compiled/path-to-regexp";

import { RESERVED_ORGANIZATION_SLUGS, RESERVED_PROJECT_SLUGS, isSlug } from "@/lib/slug";
import { config } from "@/proxy";

const APP = __dirname;

// The first path segments this console serves itself: every directory under
// `app/`, looking through route groups, that is not a dynamic segment or a
// private folder.
function ownSegments(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .flatMap((entry) =>
      entry.name.startsWith("(") ? ownSegments(path.join(dir, entry.name)) : [entry.name],
    )
    .filter((name) => !name.startsWith("[") && !name.startsWith("_"));
}

// ⚠ An organization's pages begin at `/{slug}`, beside the console's own. A
// page added here under a word auth would still hand an organization takes
// that organization's address: the static route wins, and it cannot be opened.
describe("the console's own paths", () => {
  it("are words no organization may go by", () => {
    const segments = ownSegments(APP);
    expect(segments).toContain("account");
    for (const segment of segments.filter(isSlug)) {
      expect(
        RESERVED_ORGANIZATION_SLUGS.has(segment),
        `/${segment} is served here and is not reserved in crates/shared/src/slug.rs`,
      ).toBe(true);
    }
  });
});

// ⚠ An organization's own pages sit beside its projects. A page added under a
// word auth would still hand a project takes that project's address, and the
// rail draws the page as the project.
describe("an organization's own pages", () => {
  it("are words no project may go by", () => {
    const pages = ownSegments(path.join(APP, "(app)", "[organization]"));
    expect(pages).toContain("settings");
    for (const page of pages) {
      expect(
        RESERVED_PROJECT_SLUGS.has(page),
        `/{organization}/${page} is served here and is not reserved in crates/shared/src/slug.rs`,
      ).toBe(true);
    }
  });
});

// ⚠ The proxy's matcher is a regex, and a dot in one is any character: as
// `icon.svg`, the exclusion for the icon also matched `/icon-svg`, a slug an
// organization may go by, and its pages skipped the proxy — no sign-in
// redirect, no Content Security Policy, no organization read off the path.
// Compiled here as Next compiles it.
describe("the proxy's matcher", () => {
  const compile = pathToRegexp as (pattern: string) => RegExp;
  const matchers = config.matcher.map(compile);
  const proxied = (path: string) => matchers.some((matcher) => matcher.test(path));

  it.each(["icon-svg", "favicon-ico", "apple-icon-png", "opengraph-image-png", "acme"])(
    "runs for an organization that goes by %s",
    (slug) => {
      expect(isSlug(slug) && !RESERVED_ORGANIZATION_SLUGS.has(slug)).toBe(true);
      expect(proxied(`/${slug}`)).toBe(true);
      expect(proxied(`/${slug}/settings`)).toBe(true);
    },
  );

  it.each([
    "/icon.svg",
    "/favicon.ico",
    "/apple-icon.png",
    "/opengraph-image.png",
    "/_next/static/chunk.js",
  ])("skips the asset %s", (path) => {
    expect(proxied(path)).toBe(false);
  });
});
