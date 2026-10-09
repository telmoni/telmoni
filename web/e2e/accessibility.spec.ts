import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import type { SessionData } from "@/lib/auth/session";
import { buildConsoleNav } from "@/lib/console-nav";
import { expectProjectOverview, landOnProject } from "./helpers";

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

test.describe("Accessibility Audit (WCAG 2.1 AA)", () => {
  test("console routes pass WCAG 2.1 AA accessibility checks", async ({
    page,
  }, testInfo) => {
    test.setTimeout(60_000);

    const userId = `test-user-e2e-a11y-${testInfo.project.name}`;
    await injectSession(page, {
      userId,
      email: `e2e-a11y-${testInfo.project.name}@example.com`,
      firstName: "E2E",
      lastName: "A11y",
      accessToken: "test-token",
      refreshToken: null,
      expiresAt: Date.now() + 60 * 60 * 1000,
      idToken: null,
    });

    const landing = await landOnProject(page);
    await expectProjectOverview(page);
    expect(landing, "the door lands on a project").toMatch(/^\/[a-z0-9-]+\/[a-z0-9-]+$/);
    const [, organization] = landing.split("/");

    const rows = (pathname: string) =>
      buildConsoleNav(pathname).flatMap((group) => group.items.map((item) => item.url));
    const routesToAudit = [...rows(landing), ...rows(`/${organization}`)];
    expect(routesToAudit.length, "the rail is the list").toBeGreaterThan(4);
    // Under `next dev` a route compiles on its first visit, while the suite's
    // other workers load pages too, so the budget grows with the rail: the
    // minute above for the way in, and a share per route, rather than one
    // figure that runs out as rows are added.
    test.setTimeout(60_000 + routesToAudit.length * 20_000);

    for (const route of routesToAudit) {
      const response = await page.goto(route);
      await page.waitForLoadState("domcontentloaded");
      expect(response?.status(), `${route} did not render`).toBe(200);

      const accessibilityScanResults = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
        .analyze();

      expect(
        accessibilityScanResults.violations.length,
        `Accessibility violations found on ${route}: ${JSON.stringify(
          accessibilityScanResults.violations,
          null,
          2
        )}`
      ).toBe(0);
    }
  });
});
