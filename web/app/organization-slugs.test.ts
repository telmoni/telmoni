import { readdirSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { RESERVED_ORGANIZATION_SLUGS, RESERVED_PROJECT_SLUGS, isSlug } from "@/lib/slug";

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
