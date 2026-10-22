import { test, expect, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";
import { landOnProject } from "./helpers";

// A monitor made on the Monitors page, the project's ping key minted from its
// menu, its runs reported as a crontab's `curl` reports them, and read back on
// the list and on the monitor's own page; then edited, paused, resumed and
// deleted. The pings go to the suite's own server, whose ping lane shares its
// port with the console's lanes.

const SERVER = "http://localhost:8082";
const OWNER: Partial<SessionData> = {
  userId: "test-user-e2e-monitors",
  email: "e2e-monitors@example.com",
  firstName: "Monitor",
  lastName: "Owner",
};
const ONE_SCREEN = "a monitor is the same on any screen";

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/v1/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

// A ping key the test can use, shown once: the older of two live keys from an
// earlier run retired first, since a project holds two at most.
async function mintPingKey(page: Page): Promise<string> {
  const menu = page.getByRole("button", { name: /Ping key/ });
  await menu.click();
  const retire = page.getByRole("menuitem", { name: "Retire the older key now" });
  if (await retire.isVisible().catch(() => false)) {
    await retire.click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Retire key" }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await menu.click();
  }
  await page
    .getByRole("menuitem", { name: /^(Mint the ping key|Rotate: mint a second key)$/ })
    .click();
  const shown = page.getByRole("dialog", { name: "Ping key minted" });
  const key = (await shown.locator("pre").innerText()).trim();
  await shown.getByRole("button", { name: "I've saved it" }).click();
  return key;
}

/** A section of the monitor's page, by its heading. */
function section(page: Page, name: string) {
  return page.locator("section").filter({ has: page.getByRole("heading", { name, exact: true }) });
}

test.describe("Monitors", () => {
  test("a monitor made on its page hears its pings, and is edited, paused and deleted", async ({
    page,
    request,
  }, testInfo) => {
    test.skip(testInfo.project.name !== "chromium", ONE_SCREEN);
    // Each page compiles on its first visit under `next dev`.
    test.setTimeout(180_000);
    await injectSession(page, OWNER);
    const project = await landOnProject(page);
    const name = `Nightly ${Date.now().toString(36)}`;
    const slug = name.toLowerCase().replace(" ", "-");

    await page.goto(`${project}/monitors`);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Monitors");
    const key = await mintPingKey(page);
    expect(key).toMatch(/^telmoniping_[0-9A-Za-z]{32}$/);

    // Made in the side panel: the slug follows the name, and the next due
    // times are read as the line is typed.
    await page.getByRole("button", { name: /New monitor/ }).click();
    const panel = page.getByRole("dialog", { name: "New monitor" });
    await panel.getByLabel("Name").fill(name);
    await expect(panel.getByLabel("Slug")).toHaveValue(slug);
    await panel.getByLabel("Cron line").fill("0 2 * * *");
    await expect(panel.getByText(/^Next due/)).toBeVisible();
    await panel.getByRole("button", { name: "Create monitor" }).click();
    const made = page.getByRole("dialog", { name: `${name} is watching` });
    await expect(made).toContainText(`/ping/v1/$TELMONI_PING_KEY/${slug}/start`);
    await expect(made).not.toContainText(key);
    // By the keyboard: under `next dev` its tools' button sits over the
    // panel's last corner.
    await made.getByRole("button", { name: "Done" }).press("Enter");

    const row = page.getByRole("row").filter({ hasText: name });
    await expect(row).toContainText("every day at 02:00");
    await expect(row).toContainText("Healthy");

    // A crontab's two pings: the run opens, and the page draws it again;
    // then it ends well.
    const run = crypto.randomUUID();
    const start = await request.get(`${SERVER}/ping/v1/${key}/${slug}/start?rid=${run}`);
    expect(start.status(), await start.text()).toBe(200);
    await expect(row).toContainText("Running");
    const end = await request.get(`${SERVER}/ping/v1/${key}/${slug}?rid=${run}`);
    expect(end.status(), await end.text()).toBe(200);
    // A second nudge within the console's live hold draws as the hold ends.
    await expect(row).toContainText("Healthy", { timeout: 45_000 });
    await expect(row).toContainText("OK");

    // Its page: its last success, and the run in its table once the outbox
    // has landed it.
    await row.getByRole("link", { name }).click();
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);
    await expect(page.getByText("Last success").locator("xpath=../..")).not.toContainText(
      "None yet",
    );
    await expect
      .poll(
        async () => {
          await page.reload();
          return section(page, "Last runs").getByRole("link", { name: run.slice(0, 8) }).count();
        },
        { timeout: 30_000, intervals: [1_000, 2_000, 5_000] },
      )
      .toBe(1);

    // Edited, its slug fixed; the step cap is the monitor's.
    await page.getByRole("button", { name: /^Edit/ }).click();
    const edit = page.getByRole("dialog", { name: `Edit ${name}` });
    await expect(edit.getByLabel("Slug")).toHaveCount(0);
    await edit.getByLabel("Step cap").fill("40");
    await edit.getByRole("button", { name: "Save" }).click();
    await expect(edit).toHaveCount(0);
    await expect(section(page, "Definition")).toContainText("40 steps, from the monitor");

    // Paused, and watched again.
    await page.getByRole("button", { name: "Pause" }).click();
    await expect(page.getByRole("button", { name: "Resume" })).toBeVisible();
    await page.getByRole("button", { name: "Resume" }).click();
    await expect(page.getByRole("button", { name: "Pause" })).toBeVisible();
    const changes = section(page, "Recent changes");
    await expect(changes).toContainText("Created");
    await expect(changes).toContainText("Changed its step cap");
    await expect(changes).toContainText("Paused");
    await expect(changes).toContainText("Resumed");

    // Deleted once its slug is typed, and gone from the list.
    await page.getByRole("button", { name: "Delete" }).click();
    const confirm = page.getByRole("alertdialog");
    await confirm.getByRole("textbox").fill(slug);
    await confirm.getByRole("button", { name: "Delete monitor" }).click();
    await expect(page).toHaveURL(new RegExp(`${project}/monitors$`));
    await expect(page.getByRole("link", { name })).toHaveCount(0);
  });
});
