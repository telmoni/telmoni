import { expect, type Page } from "@playwright/test";

// A fresh organization is born named after its owner and empty: `/console`
// lands on its overview, which lists its projects and is where the first one
// is made. Make one as a person would, once per user, and answer the path of
// the project the overview opens.
export async function landOnProject(page: Page): Promise<string> {
  await page.goto("/console");
  await page.waitForURL((url) => /^\/[a-z0-9-]+$/.test(url.pathname));
  const overview = new URL(page.url()).pathname;
  const projects = page.getByRole("region", { name: "Projects" });
  await expect(projects).toBeVisible();
  const first = projects.getByRole("link").first();
  if (await first.isVisible().catch(() => false)) {
    await first.click();
  } else {
    await page.getByRole("button", { name: "New project" }).click();
    await page.getByLabel("Project name").fill("Web");
    await page.getByRole("button", { name: "Create project" }).click();
  }
  await page.waitForURL(
    (url) =>
      url.pathname.startsWith(`${overview}/`) && /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname),
  );
  const landing = new URL(page.url()).pathname;
  expect(landing).not.toBe("/console");
  return landing;
}
