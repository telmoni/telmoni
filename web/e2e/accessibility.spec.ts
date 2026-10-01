import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import type { SessionData } from "@/lib/auth/session";
import { buildConsoleNav } from "@/lib/console-nav";

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

    await page.goto("/console");
    await expect(
      page.getByRole("heading", { name: "Overview", level: 1 }),
    ).toBeVisible();
    const projectId = new URL(page.url()).pathname.slice(1);
    expect(projectId, "the door lands on a project").toMatch(/^project_[A-Za-z0-9]{16}$/);

    const rows = (pathname: string, segment: string) =>
      buildConsoleNav(pathname, segment).flatMap((group) =>
        group.items.map((item) => item.url),
      );
    const routesToAudit = [
      ...rows(`/${projectId}`, projectId),
      ...rows("/organization", "organization"),
    ];
    expect(routesToAudit.length, "the rail is the list").toBeGreaterThan(4);

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
