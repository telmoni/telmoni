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
  }
  // The Projects page's own path has a project's shape, so it is ruled out.
  await page.waitForURL(
    (url) =>
      url.pathname !== listing &&
      url.pathname.startsWith(`${overview}/`) &&
      /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname),
  );
  const landing = new URL(page.url()).pathname;
  expect(landing).not.toBe("/console");
  return landing;
}

// A project's overview is titled by the project, so landing on one is the
// page's title naming the project the header's switcher stands in — read once
// the switcher has caught up with a client-side navigation, when its prompt
// for a project has given way to one's name.
export async function expectProjectOverview(page: Page): Promise<void> {
  const project = page.getByRole("button", { name: "Select a project" });
  await expect(project.getByTestId("project-prompt")).toHaveCount(0);
  const name = (await project.innerText()).trim();
  expect(name, "the switcher names the project").not.toBe("");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);
}
