import { describe, expect, it } from "vitest";

import {
  DISCUSSIONS_URL,
  PRODUCT_DESCRIPTION,
  REPO_URL,
  SPONSORS_URL,
} from "./site";

// ⚠ **Every name here has to be one `site.ts` actually exports, and only
// typecheck says so.** A constant dropped from there imports as `undefined`,
// `RegExp.test` stringifies that to "undefined", and every pattern below
// clears it — so this suite reports green on copy that is not there instead of
// failing. That is exactly how the tagline read while it was briefly gone,
// which is why it is worth saying here and not just knowing.
const CUSTOMER_COPY = { PRODUCT_DESCRIPTION };

const REFUSED: [string, RegExp][] = [
  ["the sandbox, which is dead and another product", /\b(sandbox|virtual machine)\b/i],
  ["a launch nothing has shipped", /\b(now available|generally available|today)\b/i],
];

describe("the copy a search engine quotes", () => {
  it.each(Object.entries(CUSTOMER_COPY))(
    "%s names nothing this tree deleted",
    (name, copy) => {
      for (const [claim, pattern] of REFUSED) {
        expect(pattern.test(copy), `${name} claims ${claim}: ${copy}`).toBe(
          false,
        );
      }
    },
  );

  it("says what this IS, in the nouns of what exists", () => {
    expect(PRODUCT_DESCRIPTION).toMatch(/\b(organizations?|projects?|roles?|audit)\b/i);
  });

  it("keeps the description inside what a search result shows", () => {
    expect(PRODUCT_DESCRIPTION.length).toBeLessThanOrEqual(160);
  });
});

describe("the GitHub links", () => {
  it("point at the public repository, with discussions off until they are enabled", () => {
    expect(REPO_URL).toBe("https://github.com/telmoni/telmoni");
    expect(DISCUSSIONS_URL).toBeNull();
  });

  it("allows sponsorship as the one GitHub link, and it sells nothing", () => {
    if (SPONSORS_URL === null) return;
    expect(SPONSORS_URL).toMatch(/^https:\/\/github\.com\/sponsors\//);
  });
});

