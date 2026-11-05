import { expect, type Page } from "@playwright/test";

// A fresh organization is born named after its owner and empty: `/console`
// lands on its Projects page, where the first one is made. Make one as a
// person would, once per user, and answer the path of the project that page
// opens.
export async function landOnProject(page: Page): Promise<string> {
  await page.goto("/console");
  await page.waitForURL((url) => /^\/[a-z0-9-]+\/projects$/.test(url.pathname));
  const listing = new URL(page.url()).pathname;
  const overview = listing.slice(0, -"/projects".length);
  // The Projects page's own path has a project's shape, so it is ruled out.
  const onProject = (url: URL) =>
    url.pathname !== listing &&
    url.pathname.startsWith(`${overview}/`) &&
    /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname);
  await page.goto(listing);
  const projects = page.getByRole("region", { name: "Projects" });
  await expect(projects).toBeVisible();
  const first = projects.getByRole("link").first();
  if (await first.isVisible().catch(() => false)) {
    await first.click();
  } else {
    await page.getByRole("button", { name: "New project" }).click();
    await page.getByLabel("Project name").fill("Web");
    await page.getByRole("button", { name: "Create project" }).click();
    // A test signed in as the same person may have made it a moment earlier:
    // the name is then taken, and theirs is the project to open.
    const taken = page.getByRole("alert").filter({ hasText: "already exists" });
    const refused = await Promise.race([
      taken.waitFor().then(() => true, () => false),
      page.waitForURL(onProject).then(() => false, () => false),
    ]);
    if (refused) {
      await page.goto(listing);
      await first.click();
    }
  }
  await page.waitForURL(onProject);
  const landing = new URL(page.url()).pathname;
  expect(landing).not.toBe("/console");
  return landing;
}

// A project opens on its agents, so landing on one is that page's title, and
// the header's switcher standing in a project, once it has caught up with a
// client-side navigation.
export async function expectProjectAgents(page: Page): Promise<void> {
  await expect(page.getByRole("heading", { level: 1, name: "Agents" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Project: / }),
    "the switcher names the project",
  ).toBeVisible();
}
