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
    await page.goto("/organization/members");
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
    await page.goto("/organization/members");

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
