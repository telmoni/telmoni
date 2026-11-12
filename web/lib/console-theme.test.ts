import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

const WEB = path.join(__dirname, "..");

const CSS = readFileSync(path.join(WEB, "app", "globals.css"), "utf8");

function block(selector: string): string {
  const at = CSS.indexOf(`${selector} {`);
  expect(at, `${selector} is missing from globals.css`).toBeGreaterThan(-1);
  return CSS.slice(at, CSS.indexOf("\n}", at));
}

// Keyboard focus is the browser's own outline; the console draws no ring of
// its own, and hides the outline only on an explicit saved choice.
describe("the focus indicator", () => {
  const RULE = "html.a11y-no-focus-rings *:focus-visible";

  it("is the browser's outline, with no ring drawn over it", () => {
    const rules = CSS.replace(/\/\*[\s\S]*?\*\//g, "");
    expect(rules, "a focus rule draws a ring of its own").not.toMatch(
      /:focus-visible[^{]*\{[^}]*box-shadow/,
    );
    expect(rules, "a focus rule replaces the browser's outline").not.toMatch(
      /:focus-visible[^{]*\{[^}]*outline:\s*\d/,
    );
  });

  it("hides keyboard focus only when it was explicitly turned off", () => {
    expect(block(RULE)).toMatch(/outline:\s*none/);
  });

  // Tailwind's `focus-visible:outline-*` utilities sit in `@layer utilities`
  // and beat any layered rule regardless of specificity, so a layered opt-out
  // would lose to the first component that names one.
  //
  // Brace depth, not indentation: an earlier version of this test matched
  // the rule at the start of a line and passed happily with it nested one
  // line under `@layer base {`.
  it("sits outside @layer, or the utilities would win", () => {
    const at = CSS.indexOf(`${RULE} {`);
    const before = CSS.slice(0, at).replace(/\/\*[\s\S]*?\*\//g, "");
    let depth = 0;
    for (const ch of before) {
      if (ch === "{") depth += 1;
      else if (ch === "}") depth -= 1;
    }
    expect(depth, "the rule is nested inside a block, so it is layered").toBe(
      0,
    );
  });
});

// Every control that stands on the console's own background, as opposed to on
// a card, a popover or a menu, and fills on hover. The rail's rows are the
// reference — they are what the header's four icon buttons have to match —
// so the list is both halves and the assertion is that they agree. The
// switcher (`resource-selector.tsx`) is not here: it draws no fill at all,
// because a fill around the organization's name reads as the name shifting.
const CHROME = [
  "components/console-header.tsx",
  "components/agent-button.tsx",
  "components/search-button.tsx",
  "components/notifications-bell.tsx",
  "components/nav-user.tsx",
  "components/console-sidebar.tsx",
].map((file) => ({
  file,
  // Comments discuss the classes they replaced, by name. Reading them as code
  // would make the rule unstatable in the one place it most needs explaining.
  src: readFileSync(path.join(WEB, file), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, ""),
}));

describe("the console chrome's hover fill", () => {
  // The premise of the shared token. The rail stands a step under the page the
  // header is on, so the shell reads in layers, and one fill serves both only
  // while it stands outside the pair: darker than either in light, lighter
  // than either in dark. A fill between them, or on one, is the assumption to
  // break loudly rather than the styling to quietly keep.
  it("is one colour, clear of both the rail and the page the header stands on", () => {
    for (const selector of [".theme-console", ".dark .theme-console"]) {
      const scope = block(selector);
      const lightness = (token: string) =>
        Number(new RegExp(`--${token}:\\s*oklch\\(\\s*([\\d.]+)`).exec(scope)?.[1]);
      const page = lightness("background");
      const rail = lightness("sidebar");
      const fill = lightness("sidebar-accent");
      expect(rail, `${selector}: the rail is no step under the page`).toBeLessThan(page);
      expect(
        (fill - page) * (fill - rail),
        `${selector}: the fill sits between the rail and the page, or on one of them`,
      ).toBeGreaterThan(0);
    }
  });

  it("is the one thing every chrome control reaches for", () => {
    for (const { file, src } of CHROME) {
      expect(src, `${file} does not hover with the rail's token`).toMatch(
        /(?:^|[\s"'`])(?:md:)?(?:group-)?hover:bg-sidebar-accent(?![\w-])/m,
      );
    }
  });

  // `accent` is the token for a control on a card or in a menu, and against
  // the console's background it lands at a different strength in each theme —
  // which is how the header's icon buttons came to hover fainter than the rail
  // in light and heavier in dark.
  it("is never `accent`, which is tuned for a surface these controls are not on", () => {
    for (const { file, src } of CHROME) {
      expect(
        src,
        `${file} tints with accent, which does not match the rail`,
      ).not.toMatch(/(?:^|[\s"'`])(?:group-)?hover:bg-accent(?![\w-])/m);
    }
  });

  it("would look different if it were, which is why the rule is worth a test", () => {
    for (const selector of [".theme-console", ".dark .theme-console"]) {
      const scope = block(selector);
      const accent = /--accent:\s*(oklch\([^)]*\))/.exec(scope)?.[1];
      const sidebarAccent = /--sidebar-accent:\s*(oklch\([^)]*\))/.exec(scope)?.[1];
      expect(accent, `${selector} sets no --accent`).toBeDefined();
      expect(sidebarAccent, `${selector} sets no --sidebar-accent`).toBeDefined();
      expect(
        sidebarAccent,
        `${selector}: the two tokens are equal, so this whole rule is a no-op`,
      ).not.toBe(accent);
    }
  });
});

// Sonner draws the toast, its buttons and its close button from its own
// stylesheet, on elements that carry no `rounded` class, so the sharp theme's
// attribute rule never reached them: a sharp console showed a rounded toast.
describe("the sharp corner theme", () => {
  const SONNER_SHARP =
    ".theme-sharp .toaster [data-sonner-toast],\n" +
    ".theme-sharp .toaster [data-sonner-toast] [data-button],\n" +
    ".theme-sharp .toaster [data-sonner-toast] [data-close-button],\n" +
    ".theme-sharp .toaster .sonner-loading-bar";

  it("squares every class that rounds", () => {
    expect(block('.theme-sharp [class*="rounded"]')).toMatch(/border-radius:\s*0/);
  });

  it("reaches the toasts sonner draws without a rounded class", () => {
    expect(block(SONNER_SHARP)).toMatch(/border-radius:\s*0/);
  });

  // The sharp rule wins by specificity alone. An important radius on a toast
  // class would outrank it, and sonner's action button once carried one.
  it("is never outranked by an important radius on the toaster", () => {
    // Comments discuss the class by name; only code counts.
    const src = readFileSync(
      path.join(WEB, "components", "ui", "sonner.tsx"),
      "utf8",
    )
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "");
    expect(src, "a !rounded-* class on the toaster beats the sharp theme").not.toMatch(
      /!rounded/,
    );
  });
});
