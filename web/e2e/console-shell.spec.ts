import { test, expect, type Locator, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";
import { landOnProject } from "./helpers";

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/v1/test/session", { data });
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
  const [, organization] = (await landOnProject(page)).split("/");
  await page.goto(`/${organization}/members`);
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
  test("hangs the brand and the toggle on the rail's edges, and the rows between", async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "chromium",
      "the wordmark is drawn above the cut only",
    );
    await injectSession(page, testUser(testInfo.project.name));
    await gotoOrganizationMembers(page);

    // The wordmark links home; the mark keeps its own fixed box.
    const brand = page.getByRole("banner").getByRole("link", { name: "Telmoni", exact: true });
    await expect(brand).toBeVisible();
    const rail = page.locator("#console-sidebar");
    const row = rail.getByRole("link", { name: "Projects" });

    // The brand and collapse fill the rail's cell; search stays on the right.
    const railOpen = (await rail.boundingBox())!;
    const brandOpen = (await brand.boundingBox())!;
    const rowOpen = (await row.boundingBox())!;
    const close = (await page.getByRole("button", { name: "Collapse navigation" }).boundingBox())!;
    const markIcon = page.locator('[data-slot="console-brand-mark"] svg');
    const mark = (await markIcon.boundingBox())!;
    const word = (await brand.locator("svg").last().boundingBox())!;
    const search = (await page.getByRole("button", { name: "Search", exact: true }).boundingBox())!;
    const profile = (await page.getByRole("button", { name: /^Account:/ }).boundingBox())!;
    expect(railOpen.width).toBe(252);
    expect(close.width).toBe(36);
    expect(Math.abs(mark.x + mark.width / 2 - (rowOpen.x + 18))).toBeLessThanOrEqual(1);
    expect(Math.abs(rowOpen.x - railOpen.x - 18)).toBeLessThanOrEqual(1);
    expect(Math.abs(word.x - (rowOpen.x + 44))).toBeLessThanOrEqual(1);
    expect(close.x).toBeGreaterThan(brandOpen.x + brandOpen.width);
    expect(search.x).toBeGreaterThan(railOpen.x + railOpen.width);
    expect(profile.width).toBe(36);
    expect(profile.height).toBe(36);
    expect(profile.y).toBe(search.y);
    expect(Math.abs(railOpen.x + railOpen.width - close.x - close.width - 18)).toBeLessThanOrEqual(1);

    await page.getByRole("button", { name: "Collapse navigation" }).click();
    const open = page.getByRole("button", { name: "Expand navigation" });
    await expect(open).toBeVisible();
    await expect(open).toBeFocused();
    await page.waitForTimeout(400);

    // The wordmark hides while the mark and toggle keep the rail's icon axis.
    await expect(brand).toBeHidden();
    await expect(markIcon).toBeVisible();
    expect(await markIcon.boundingBox()).toEqual(mark);
    const railShut = (await rail.boundingBox())!;
    const centre = railShut.x + railShut.width / 2;
    for (const piece of [open, row]) {
      const box = (await piece.boundingBox())!;
      expect(Math.abs(box.x + box.width / 2 - centre)).toBeLessThanOrEqual(1);
    }

    await open.click();
    await expect(brand).toBeVisible();
    await expect(page.getByRole("button", { name: "Collapse navigation" })).toBeFocused();
    await page.waitForTimeout(400);
    expect(Math.abs((await brand.boundingBox())!.x - brandOpen.x)).toBeLessThanOrEqual(1);
  });
});

test.describe("Console rail on a dead address", () => {
  // The rail is spelled from the path, and a path that names nothing would
  // spell a rail of links to more "not found". From the server's HTML: the
  // page is loaded, not navigated to.
  test("draws the way back, and none of the dead address's rows", async ({
    page,
  }, testInfo) => {
    test.setTimeout(60_000);
    await injectSession(page, testUser(testInfo.project.name));
    const [, organization] = (await landOnProject(page)).split("/");
    const rail = page.locator("#console-sidebar");
    // On a phone the closed drawer is hidden, rows and all, so it is opened
    // first: a hidden row is no row to a person, nor to `getByRole`.
    const openDrawer = async () => {
      if (testInfo.project.name !== "mobile-chromium") return;
      await page.getByRole("button", { name: "Expand navigation" }).click();
      await expect(rail).toBeVisible();
    };

    // A project's page: its own rows, and no way back to step out by.
    await openDrawer();
    await expect(rail.getByRole("link", { name: "API keys" })).toBeAttached();
    await expect(rail.getByRole("link", { name: /^Back to / })).toHaveCount(0);

    for (const dead of [
      `/${organization}/no-such-project/settings`,
      "/no-such-organization/settings",
    ]) {
      await page.goto(dead);
      await expect(
        page.getByRole("heading", { name: "Not found.", level: 1 }),
        dead,
      ).toBeVisible();
      await openDrawer();
      await expect(rail.getByRole("link", { name: "Settings" }), dead).toHaveCount(0);
      await expect(rail.getByRole("link", { name: /^Back to / }), dead).toHaveCount(1);
    }
  });
});
