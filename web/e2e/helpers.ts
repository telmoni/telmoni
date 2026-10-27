import { expect, type Page } from "@playwright/test";

// A fresh organization is born named after its owner and empty: `/console`
// lands on its overview, which is its name alone, and its projects are on its
// Projects page, where the first one is made. Make one as a person would, once
// per user, and answer the path of the project that page opens.
export async function landOnProject(page: Page): Promise<string> {
  await page.goto("/console");
  await page.waitForURL((url) => /^\/[a-z0-9-]+$/.test(url.pathname));
  const overview = new URL(page.url()).pathname;
  const listing = `${overview}/projects`;
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

// A project's overview is titled by the project, so landing on one is the
// page's title naming the project the header's switcher stands in, once the
// switcher has caught up with a client-side navigation.
export async function expectProjectOverview(page: Page): Promise<void> {
  const name = (await page.getByRole("heading", { level: 1 }).innerText()).trim();
  expect(name, "the page has a title").not.toBe("");
  await expect(
    page.getByRole("button", { name: `Project: ${name}`, exact: true }),
    "the switcher names the project",
  ).toBeVisible();
}
