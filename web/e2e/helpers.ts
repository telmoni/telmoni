import { expect, type Page } from "@playwright/test";

// A fresh organization is born unnamed and empty: `/console` asks its owner
// for a name, then lands on its Projects page until a project exists. Do both
// as a person would, once per user, and answer the project's path.
export async function landOnProject(page: Page): Promise<string> {
  await page.goto("/console");
  const nameBox = page.getByLabel("Organization name");
  if (await nameBox.isVisible().catch(() => false)) {
    await nameBox.fill("Acme");
    await page.getByRole("button", { name: "Continue" }).click();
  }
  await page.waitForURL((url) => /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname));
  const landed = new URL(page.url()).pathname;
  if (landed.endsWith("/projects")) {
    await page.getByRole("button", { name: "New project" }).click();
    await page.getByLabel("Project name").fill("Web");
    await page.getByRole("button", { name: "Create project" }).click();
    // The Projects page has a project's shape too: wait to leave it.
    await page.waitForURL(
      (url) => url.pathname !== landed && /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname),
    );
  }
  const landing = new URL(page.url()).pathname;
  expect(landing).not.toBe("/console");
  return landing;
}
