import { test, expect, type APIRequestContext, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";
import { buildConsoleNav } from "@/lib/console-nav";
import { expectProjectOverview, landOnProject } from "./helpers";

const SERVICE_SECRET = "e2e-service-secret-do-not-use-in-production";
const SERVER = "http://localhost:8082";
const LIST_CARD = ["overflow-hidden", "p-0", "gap-0"];

type TestUser = { userId: string; email: string; firstName: string; lastName: string };

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

function testUser(project: string): TestUser {
  return {
    userId: `test-user-e2e-containers-${project}`,
    email: `e2e-containers-${project}@example.com`,
    firstName: "E2E",
    lastName: "Containers",
  };
}

// A bearer for `user` in a session of its own, from auth's test door — the
// one the console's door asks — so a request made straight to the server
// from here is the same person the browser is. The door records the identity
// it is given, so it is given the browser's.
async function bearerFor(request: APIRequestContext, user: TestUser): Promise<string> {
  const res = await request.post(`${SERVER}/test/session`, {
    headers: { "x-service-secret": SERVICE_SECRET },
    data: user,
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  const { accessToken } = (await res.json()) as { accessToken: string };
  return accessToken;
}

function internalHeaders(
  bearer: string,
  organizationId: string,
  projectId?: string,
): Record<string, string> {
  return {
    authorization: `Bearer ${bearer}`,
    "x-service-secret": SERVICE_SECRET,
    "x-organization-id": organizationId,
    ...(projectId ? { "x-project-id": projectId } : {}),
  };
}

// The organization the console's first visit provisioned for this person,
// asked under a session of its own: `/me` records a row for every session it
// sees, and the seed revokes every row it lists, so the probe is one of them.
async function organizationOf(request: APIRequestContext, user: TestUser): Promise<string> {
  const bearer = await bearerFor(request, user);
  const res = await request.post(`${SERVER}/me`, {
    headers: { authorization: `Bearer ${bearer}`, "x-service-secret": SERVICE_SECRET },
    data: {},
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  const { activeOrganizationId } = (await res.json()) as { activeOrganizationId: string };
  expect(activeOrganizationId).toMatch(/^org_[A-Za-z0-9]{16}$/);
  return activeOrganizationId;
}

// Leaves the user with one API key and no live session. The row the privacy
// page then lists is recorded by the next console visit's `/me` — which is why
// the caller injects a fresh session afterwards: this ends the browser's own.
// The seed acts under a session `/me` never saw, so no row it revokes is its own.
async function seed(request: APIRequestContext, user: TestUser, projectSlug: string) {
  const organizationId = await organizationOf(request, user);
  const bearer = await bearerFor(request, user);
  const own = internalHeaders(bearer, organizationId);
  // The page's path names the project by slug; its lanes key on the id.
  const listing = await request.get(`${SERVER}/internal/projects`, { headers: own });
  expect(listing.ok(), await listing.text()).toBeTruthy();
  const { projects } = (await listing.json()) as { projects: { id: string; slug: string }[] };
  const projectId = projects.find((p) => p.slug === projectSlug)?.id;
  expect(projectId, `no project goes by ${projectSlug}`).toMatch(/^project_[A-Za-z0-9]{16}$/);
  const project = internalHeaders(bearer, organizationId, projectId);

  const tokens = await request.get(`${SERVER}/internal/tokens`, { headers: project });
  expect(tokens.ok(), await tokens.text()).toBeTruthy();
  for (const t of (await tokens.json()) as { id: string }[]) {
    expect((await request.delete(`${SERVER}/internal/tokens/${t.id}`, { headers: project })).ok()).toBeTruthy();
  }
  const sessions = await request.get(`${SERVER}/internal/auth/sessions`, { headers: own });
  expect(sessions.ok(), await sessions.text()).toBeTruthy();
  for (const s of ((await sessions.json()) as { sessions: { id: string }[] }).sessions) {
    expect(
      (await request.post(`${SERVER}/internal/auth/sessions/${s.id}/revoke`, { headers: own })).ok(),
    ).toBeTruthy();
  }

  const minted = await request.post(`${SERVER}/internal/tokens`, {
    headers: project,
    data: { name: "e2e layout key", created_by: user.userId },
  });
  expect(minted.ok(), await minted.text()).toBeTruthy();
}

type Snapshot = {
  children: number;
  wrappers: number;
  wrapper: { tag: string; classes: string[] } | null;
  wrapperIsLast: boolean;
  lead: string[] | null;
  sections: string[][];
  tables: {
    card: string[] | null;
    rows: number;
    cells: string[][];
    reachable: boolean;
    cardScrolls: boolean;
    cardScrollWidth: number;
    squeezed: number;
    right: number;
  }[];
  emptyStates: (string[] | null)[];
  main: { width: number };
  pane: { clientWidth: number; scrollWidth: number };
};

function snapshot(page: Page): Promise<Snapshot> {
  return page.evaluate(() => {
    const cls = (el: Element | null) => (el ? Array.from(el.classList) : null);
    const main = document.querySelector("#console-main");
    if (!main) throw new Error("no #console-main on the page");
    const pane = main.closest('[data-slot="scroll-area-viewport"]');
    if (!pane) throw new Error("<main> is not inside the scrollport");
    const kids = Array.from(main.children);
    const wrappers = kids.filter(
      (el) =>
        el.tagName === "DIV" &&
        el.classList.contains("grid") &&
        el.classList.contains("gap-6"),
    );
    const content = wrappers[0] ?? null;
    return {
      children: kids.length,
      wrappers: wrappers.length,
      wrapper: content
        ? { tag: content.tagName, classes: Array.from(content.classList) }
        : null,
      wrapperIsLast: content ? kids[kids.length - 1] === content : false,
      lead: kids[0] && kids[0] !== content ? cls(kids[0]) : null,
      sections: Array.from(main.querySelectorAll("section")).map((s) => Array.from(s.classList)),
      tables: Array.from(main.querySelectorAll("table")).map((t) => {
        const card = t.closest('[data-slot="card"]');
        const box = t.getBoundingClientRect();
        const cardBox = card?.getBoundingClientRect();
        const cells = Array.from(t.querySelectorAll("th, td"));
        return {
          card: cls(card),
          rows: t.tBodies[0]?.rows.length ?? 0,
          cells: cells.map((c) => Array.from(c.classList)),
          reachable:
            !cardBox ||
            box.right <= cardBox.right + 1 ||
            (card as HTMLElement).scrollWidth > (card as HTMLElement).clientWidth,
          squeezed: cells.filter(
            (c) => c.classList.contains("whitespace-nowrap") && c.scrollWidth > c.clientWidth + 1,
          ).length,
          cardScrolls: card
            ? (card as HTMLElement).scrollWidth > (card as HTMLElement).clientWidth
            : false,
          // Rounded, because `scrollWidth` below is an integer while a
          // bounding box is fractional: a table 0.05px past a whole pixel
          // is not one the card cannot scroll to.
          right: cardBox
            ? Math.round(box.right - cardBox.left + (card as HTMLElement).scrollLeft)
            : 0,
          cardScrollWidth: card ? (card as HTMLElement).scrollWidth : 0,
        };
      }),
      emptyStates: Array.from(main.querySelectorAll("div.grid.gap-1.p-4")).map((d) =>
        cls(d.closest('[data-slot="card"]')),
      ),
      main: { width: main.getBoundingClientRect().width },
      pane: { clientWidth: pane.clientWidth, scrollWidth: pane.scrollWidth },
    };
  });
}

test.describe("Console content containers", () => {
  test("every page draws its content on the standard containers, at every width", async ({
    page,
  }, testInfo) => {
    test.setTimeout(150_000);
    const mobile = testInfo.project.name === "mobile-chromium";
    const user = testUser(testInfo.project.name);
    await injectSession(page, user);

    const landing = await landOnProject(page);
    await expectProjectOverview(page);
    expect(landing, "the door lands on a project").toMatch(/^\/[a-z0-9-]+\/[a-z0-9-]+$/);
    const [, organization, projectSlug] = landing.split("/");
    await seed(page.request, user, projectSlug!);
    // `seed` revoked every session, this browser's included. A fresh one, and
    // the `/me` its first visit makes, is the single row the privacy page lists.
    await injectSession(page, user);
    await landOnProject(page);
    await expectProjectOverview(page);

    const rows = (pathname: string) =>
      buildConsoleNav(pathname).flatMap((group) => group.items.map((item) => item.url));
    const routes = [
      ...rows(landing),
      ...rows(`/${organization}`),
      // Not in the rail — it hangs off the account menu — but it is where the
      // active-sessions table lives, which is what this spec seeds a session for.
      "/account/privacy",
    ];
    expect(routes.length).toBeGreaterThan(8);

    const seen = new Map<string, Snapshot>();
    for (const route of routes) {
      const response = await page.goto(route);
      expect(response?.status(), `${route} did not render`).toBe(200);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      const s = await snapshot(page);
      seen.set(route, s);
      const at = (what: string) => `${route}: ${what}`;

      if (s.children > 0) {
        // <main> is `flex flex-col gap-3` (console-shell.tsx), and every page
        // draws an optional `PageHeader` title row followed by one content
        // wrapper. The row is a sibling rather than a child of the wrapper so
        // the gap above the content stays main's gap-3, and so the header's
        // action lines up with the cards — which console-shell.spec.ts pins.
        expect(s.wrappers, at("one content wrapper under <main>")).toBe(1);
        expect(s.wrapper?.tag, at("the wrapper is a div")).toBe("DIV");
        expect(s.wrapper?.classes, at("the wrapper is grid gap-6")).toEqual(
          expect.arrayContaining(["grid", "gap-6"]),
        );
        expect(s.wrapperIsLast, at("the wrapper is main's last child")).toBe(true);
        expect(
          s.children,
          at("only the title row may precede the wrapper"),
        ).toBeLessThanOrEqual(2);
        if (s.lead) {
          expect(s.lead, at("the row before the wrapper is the title row")).toEqual(
            expect.arrayContaining(["flex", "min-h-9"]),
          );
        }
      }
      for (const section of s.sections) {
        expect(section, at("a section is grid gap-3")).toEqual(
          expect.arrayContaining(["grid", "gap-3"]),
        );
      }
      for (const t of s.tables) {
        expect(t.card, at("a table sits on the list card")).toEqual(expect.arrayContaining(LIST_CARD));
        for (const cell of t.cells) {
          expect(cell, at("a cell is px-4 py-3")).toEqual(expect.arrayContaining(["px-4", "py-3"]));
        }
        expect(t.reachable, at("the card cannot scroll to its whole table")).toBe(true);
        expect(t.squeezed, at("a nowrap cell is narrower than its text")).toBe(0);
        expect(
          t.right,
          at("the table's far edge is past where its card scrolls"),
        ).toBeLessThanOrEqual(t.cardScrollWidth + 1);
      }
      for (const card of s.emptyStates) {
        expect(card, at("an empty state sits on the list card")).toEqual(expect.arrayContaining(LIST_CARD));
      }
      expect(s.main.width, at("<main> is not the pane's width")).toBeGreaterThanOrEqual(
        s.pane.clientWidth - 1,
      );
      expect(s.pane.scrollWidth, at("the page scrolls sideways")).toBe(s.pane.clientWidth);
    }

    const keys = seen.get(`${landing}/api-keys`)!;
    expect(keys.tables.map((t) => t.rows), "API keys: the seeded key").toEqual([1]);
    const audit = seen.get(`${landing}/audit-log`)!;
    expect(audit.tables.length, "audit: one table").toBe(1);
    expect(audit.tables[0]!.rows, "audit: the mint on the project's page").toBeGreaterThanOrEqual(1);
    const organizationAudit = seen.get(`/${organization}/audit-log`)!;
    expect(organizationAudit.tables.length, "organization audit: one table").toBe(1);
    expect(
      organizationAudit.tables[0]!.rows,
      "organization audit: the mint and the pairing",
    ).toBeGreaterThanOrEqual(2);
    const settings = seen.get(`/${organization}/settings`)!;
    expect(
      settings.tables.length,
      "organization settings: no table — the name, the URL, the id row and the danger zone",
    ).toBe(0);
    const privacy = seen.get("/account/privacy")!;
    expect(privacy.tables.map((t) => t.rows), "privacy: the recorded session").toEqual([1]);
    const members = seen.get(`${landing}/members`)!;
    expect(members.tables.map((t) => t.rows), "members: the roster, with its owner on it").toEqual([1]);

    if (mobile) {
      expect(
        keys.tables.some((t) => t.cardScrolls),
        "API keys: the card scrolls to a table wider than the phone",
      ).toBe(true);
    }
  });
});
