import AxeBuilder from "@axe-core/playwright";
import { test, expect, type Page } from "@playwright/test";

async function gateFor(page: Page, path: string): Promise<string> {
  const res = await page.request.get(path, { maxRedirects: 0 });
  expect(res.status(), `${path} is gated`).toBe(307);
  return res.headers()["location"] ?? "";
}

test.describe("Unauthenticated navigation", () => {
  test("the root is the public splash — no login redirect", async ({ page }) => {
    await page.goto("/");
    await expect(page).not.toHaveURL(/\/auth\/login/);
  });

  test("a protected route redirects to login and keeps the destination", async ({
    page,
  }) => {
    const location = await gateFor(page, "/acme/~/settings");
    expect(location).toContain("/auth/login");
    expect(location).toContain("returnTo=%2Facme%2F%7E%2Fsettings");
  });

  test("/install.sh redirects to the release asset, not to login", async ({
    page,
  }) => {
    const res = await page.request.get("/install.sh", { maxRedirects: 0 });
    expect(res.status(), "the install line is not gated").toBe(307);
    const location = res.headers()["location"] ?? "";
    expect(location).toBe(
      "https://github.com/telmoni/telmoni-cli/releases/latest/download/install.sh",
    );
    expect(location, "a login page is not a shell script").not.toContain(
      "/auth/login",
    );
  });

  test("console pages are protected by default", async ({ page }) => {
    const location = await gateFor(page, "/acme/some-project/api-keys");
    expect(location).toContain("/auth/login");
  });
});

test.describe("Accessibility (public pages)", () => {
  for (const path of ["/", "/about", "/security", "/invite/invalid-token", "/legal/privacy-policy", "/legal/terms-of-service"]) {
    test(`${path} has no WCAG A/AA violations`, async ({ page }) => {
      await page.goto(path);
      const { violations } = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
        .analyze();
      expect(violations).toEqual([]);
    });
  }
});
