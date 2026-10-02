import { test, expect, type Locator, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

function testUser(project: string): Partial<SessionData> {
  return {
    userId: `test-user-e2e-shell-${project}`,
    email: `e2e-shell-${project}@example.com`,
    firstName: "E2E",
    lastName: "Shell",
    accessToken: "test-token",
  };
}

// The members page of the organization the door lands in: its path is
// spelled with the organization's slug, which only the landing gives.
async function gotoOrganizationMembers(page: Page) {
  await page.goto("/console");
  await page.waitForURL((url) => /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname));
  const [, organization] = new URL(page.url()).pathname.split("/");
  await page.goto(`/${organization}/~/members`);
}

async function rightEdge(locator: Locator): Promise<number> {
  const box = await locator.boundingBox();
  expect(box, "the element is laid out").not.toBeNull();
  return box!.x + box!.width;
}

test.describe("Console page title row", () => {
  test("its action ends where the cards end, at rest and scrolled", async ({
    page,
  }, testInfo) => {
    await injectSession(page, testUser(testInfo.project.name));
    await gotoOrganizationMembers(page);
    await expect(
      page.getByRole("heading", { name: "Members", level: 1 }),
    ).toBeVisible();

    const action = page.getByRole("button", { name: "Invite member" });
    const card = page.locator('[data-slot="card"]').last();
    const pane = page.locator('[data-slot="scroll-area-viewport"]', {
      has: page.locator("#console-main"),
    });

    expect(
      Math.abs((await rightEdge(action)) - (await rightEdge(card))),
    ).toBeLessThanOrEqual(1);

    const overhang = await pane.evaluate((el) => el.scrollWidth - el.clientWidth);
    expect(overhang, "the page scrolls sideways").toBe(0);

    expect(await rightEdge(action)).toBeLessThanOrEqual(page.viewportSize()!.width);

    await pane.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    expect(
      Math.abs((await rightEdge(action)) - (await rightEdge(card))),
    ).toBeLessThanOrEqual(1);
  });
});
test.describe("Console chrome row", () => {
  test("hangs the name and the toggle on the rail's two edges", async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "chromium",
      "the wordmark is drawn above the cut only",
    );
    await injectSession(page, testUser(testInfo.project.name));
    await gotoOrganizationMembers(page);

    const mark = page.getByRole("banner").getByText("Telmoni", { exact: true });
    await expect(mark).toBeVisible();
    const before = (await mark.boundingBox())!.x;

    const rail = page.locator("#console-sidebar");
    const toggle = page.getByRole("button", { name: /navigation$/ });

    // Expanded, the header's leading box is the rail's own width: the name
    // opens on the rail's left edge and the toggle closes on its right one.
    const railOpen = (await rail.boundingBox())!;
    const toggleOpen = (await toggle.boundingBox())!;
    expect(Math.abs(before - railOpen.x)).toBeLessThanOrEqual(1);
    expect(
      Math.abs(toggleOpen.x + toggleOpen.width - (railOpen.x + railOpen.width)),
    ).toBeLessThanOrEqual(1);

    await page.getByRole("button", { name: "Collapse navigation" }).click();
    await expect(
      page.getByRole("button", { name: "Expand navigation" }),
    ).toBeVisible();
    await page.waitForTimeout(400);

    // ⚠ **Gone, not moved.** Collapsed, the rail is a 32px icon column and the
    // name is the one piece of chrome wider than it, so it belongs to the open
    // rail rather than to the header. The toggle takes the whole column and
    // lands on the centre every rail icon shares.
    await expect(mark).toBeHidden();
    const railShut = (await rail.boundingBox())!;
    const toggleShut = (await toggle.boundingBox())!;
    expect(
      Math.abs(
        toggleShut.x + toggleShut.width / 2 - (railShut.x + railShut.width / 2),
      ),
    ).toBeLessThanOrEqual(1);

    await page.getByRole("button", { name: "Expand navigation" }).click();
    await expect(mark).toBeVisible();
    await page.waitForTimeout(400);
    expect(Math.abs((await mark.boundingBox())!.x - before)).toBeLessThanOrEqual(1);
  });
});

test.describe("Console rail on a dead address", () => {
  // The rail is spelled from the path, and a path that names nothing would
  // spell a rail of links to more "not found". From the server's HTML: the
  // page is loaded, not navigated to.
  test("draws the way back, and none of the dead address's rows", async ({
    page,
  }, testInfo) => {
    await injectSession(page, testUser(testInfo.project.name));
    await page.goto("/console");
    await page.waitForURL((url) => /^\/[a-z0-9-]+\/[a-z0-9-]+$/.test(url.pathname));
    const [, organization] = new URL(page.url()).pathname.split("/");
    const rail = page.locator("#console-sidebar");

    // A project's page: its own rows, and no way back to step out by.
    await expect(rail.getByRole("link", { name: "API keys" })).toBeAttached();
    await expect(rail.getByRole("link", { name: /^Back to / })).toHaveCount(0);

    for (const dead of [
      `/${organization}/no-such-project/settings`,
      "/no-such-organization/~/settings",
    ]) {
      await page.goto(dead);
      await expect(
        page.getByRole("heading", { name: "Not found.", level: 1 }),
        dead,
      ).toBeVisible();
      await expect(rail.getByRole("link", { name: "Settings" }), dead).toHaveCount(0);
      await expect(rail.getByRole("link", { name: /^Back to / }), dead).toHaveCount(1);
    }
  });
});
