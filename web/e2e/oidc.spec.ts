import { test, expect, type Page } from "@playwright/test";
import type { SessionData } from "@/lib/auth/session";
import { landOnProject } from "./helpers";

// Sign-in through a real OpenID Connect provider: Dex, the compose file's
// `oidc` profile (`crates/auth/dex/dev-config.yaml`), which refuses any
// sign-in without PKCE. Each one that gets through proves the console sent
// its challenge, the server redeemed the code with the verifier, and Dex's
// RS256 token came back with the sign-in's nonce. Sign-ups are closed, as a
// deployment's are unless its operator opens them: an invitation is the way in.

const ADA = "ada@example.com";
const OWNER: Partial<SessionData> = {
  userId: "test-user-e2e-oidc-owner",
  email: "e2e-oidc-owner@example.com",
  firstName: "OIDC",
  lastName: "Owner",
};

async function injectSession(page: Page, data: Partial<SessionData>) {
  const res = await page.request.post("/api/v1/test/session", { data });
  expect(res.ok()).toBeTruthy();
}

// The sign-in page's "Continue with Dex", then Dex's own form.
async function throughDex(page: Page, email: string) {
  await page.goto("/auth/login");
  await page.getByRole("link", { name: "Continue with Dex" }).click();
  await page.getByLabel("Email Address").fill(email);
  await page.getByLabel("Password", { exact: true }).fill("password");
  await page.getByRole("button", { name: "Login" }).click();
}

// Confirms the dialog a destructive row action opens.
async function confirm(page: Page, label: string) {
  await page.getByRole("alertdialog").getByRole("button", { name: label }).click();
}

// Ada out of the owner's organization, invited or seated, so a run starts
// where the last one began.
async function withoutAda(owner: Page, organization: string) {
  await owner.goto(`/${organization}/members`);
  const seated = owner.getByRole("button", { name: `Remove ${ADA}` });
  if (await seated.isVisible().catch(() => false)) {
    await seated.click();
    await confirm(owner, "Remove");
    await expect(seated).toHaveCount(0);
  }
  const invited = owner.getByRole("button", { name: `Withdraw the invitation to ${ADA}` });
  if (await invited.isVisible().catch(() => false)) {
    await invited.click();
    await confirm(owner, "Withdraw");
    await expect(invited).toHaveCount(0);
  }
}

const ONE_SCREEN = "a sign-in is the same on any screen";

test.describe("Sign-in through an OpenID Connect provider", () => {
  test("a person nobody invited is turned away, and told why", async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== "chromium", ONE_SCREEN);
    await throughDex(page, "grace@example.com");
    await expect(page).toHaveURL(/\/auth\/refused\?reason=closed$/);
    await expect(page.getByText("New people join this deployment by invitation.")).toBeVisible();
    await expect(page.getByRole("link", { name: "Try again" })).toHaveAttribute(
      "href",
      "/auth/login",
    );
  });

  test("an invited person signs in through the provider and joins", async ({
    page,
    browser,
  }, testInfo) => {
    test.skip(testInfo.project.name !== "chromium", ONE_SCREEN);
    const owner = await (await browser.newContext()).newPage();
    await injectSession(owner, OWNER);
    const organization = (await landOnProject(owner)).split("/")[1]!;
    await owner.goto(`/${organization}`);
    const name = (await owner.getByRole("heading", { level: 1 }).innerText()).trim();
    await withoutAda(owner, organization);

    await owner.getByRole("button", { name: "Invite member" }).click();
    await owner.getByLabel("Email address").fill(ADA);
    await owner.getByRole("button", { name: "Send invitation" }).click();
    await owner.getByRole("dialog", { name: "Invitation sent" }).getByRole("button", { name: "Done" }).click();
    await expect(
      owner.getByRole("button", { name: `Withdraw the invitation to ${ADA}` }),
    ).toBeVisible();

    // Dex vouches for her address, which a closed deployment admits an
    // invitation by, and which accepting it in the console needs.
    await throughDex(page, ADA);
    await expect(page).not.toHaveURL(/\/auth\//);
    await page.goto("/account/notifications");
    const invitation = page.getByRole("row", { name });
    await invitation.getByRole("button", { name: "Accept" }).click();
    await expect(invitation).toHaveCount(0);
    await page.goto(`/${organization}`);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(name);

    // Seated under the name Dex gave, its `name` claim alone.
    await owner.goto(`/${organization}/members`);
    await expect(owner.getByRole("row", { name: /Ada Lovelace/ })).toBeVisible();
    await withoutAda(owner, organization);
  });

  test("an address held to single sign-on is shown the provider, not the form", async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== "chromium", ONE_SCREEN);
    await page.goto("/auth/login");
    await page.getByLabel("Email address").fill("linus@sso.example");
    await page.locator("#password").fill("not-the-point");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(
      page.getByText("Your organization signs you in through its identity provider."),
    ).toBeVisible();
    await expect(page.getByRole("link", { name: "Continue with Dex" })).toBeVisible();
    await expect(page.locator("#password")).toHaveCount(0);
  });
});
