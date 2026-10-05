import { test, expect, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";
import { landOnProject } from "./helpers";

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

async function clearSession(page: Page) {
  const res = await page.request.delete("/api/test/session");
  expect(res.ok()).toBeTruthy();
}

async function gateFor(page: Page, path: string): Promise<string | null> {
  const res = await page.request.get(path, { maxRedirects: 0 });
  if (res.status() !== 307) return null;
  return res.headers()["location"] ?? "";
}

function testUser(project: string): Partial<SessionData> {
  return {
    userId: `test-user-e2e-auth-${project}`,
    email: `e2e-auth-${project}@example.com`,
    firstName: "E2E",
    lastName: "Auth",
    accessToken: "test-token",
  };
}

test.describe("Authentication", () => {
  test.afterEach(async ({ page }) => {
    await clearSession(page);
  });

  test("a protected route redirects to /auth/login when unauthenticated", async ({
    page,
  }) => {
    const location = await gateFor(page, "/console");
    expect(location).toContain("/auth/login");
  });

  test("the portal is reachable after injecting a test session", async ({ page }, testInfo) => {
    await injectSession(page, testUser(testInfo.project.name));
    const landing = await landOnProject(page);
    await expect(
      page.getByRole("heading", { name: "Overview", level: 1 }),
    ).toBeVisible();
    // A project's overview: its slug under its organization's.
    expect(landing).toMatch(/^\/[a-z0-9-]+\/[a-z0-9-]+$/);
  });

  test("the intended destination survives the login round-trip", async ({ page }, testInfo) => {
    const location = await gateFor(page, "/acme/settings");
    expect(location).toContain("/auth/login");
    expect(location).toContain("returnTo=%2Facme%2Fsettings");

    await injectSession(page, testUser(testInfo.project.name));
    await page.goto("/acme/settings");
    await expect(page).not.toHaveURL(/\/auth\/login/);
  });

  test("clearing the session re-gates protected routes", async ({ page }, testInfo) => {
    await injectSession(page, testUser(testInfo.project.name));
    await landOnProject(page);
    await expect(
      page.getByRole("heading", { name: "Overview", level: 1 }),
    ).toBeVisible();

    await clearSession(page);
    const location = await gateFor(page, "/console");
    expect(location).toContain("/auth/login");
  });
});
