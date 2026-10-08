import { describe, it, expect } from "vitest";
import { isPublic } from "./public-paths";

describe("isPublic", () => {
  it.each([
    "/",
    // No page behind these: the proxy redirects them to the deployment's
    // published documents, before any sign-in gate.
    "/legal/privacy-policy",
    "/legal/terms-of-service",
    "/auth",
    "/auth/login",
    "/auth/callback",
    "/auth/logout",
    "/api/health",
    "/robots.txt",
    "/sitemap.xml",
    "/v1",
    "/v1/members",
    "/cli",
    "/cli/me",
    "/cli/auth/start",
    "/docs",
    "/docs/self-host/overview",
    "/api/search",
    "/llms.txt",
    "/llms-full.txt",
  ])("treats %s as public", (p) => {
    expect(isPublic(p)).toBe(true);
  });

  it.each([
    // The operator's pages left this repository; nothing here serves them.
    "/about",
    "/security",
    "/api/project",
    "/api/healthcheck",
    "/api/members/u_123",
    "/api/project/slug",
    "/api/projects/switch",
    "/acme",
    "/acme/web",
    "/acme/web/api-keys",
    "/acme/settings",
    "/v1x",
    "/v1-internal",
    "/client",
    "/cli-internal",
    "/docsx",
    "/api/search/x",
    "/llms.txt/x",
  ])("treats %s as protected", (p) => {
    expect(isPublic(p)).toBe(false);
  });

  it("does not let a public-prefix substring leak into protected paths", () => {
    expect(isPublic("/acme/auth")).toBe(false);
  });

  it.each(["/authsecrets", "/authfoo"])(
    "rejects %s — a path that shares a prefix but lacks the boundary",
    (p) => {
      expect(isPublic(p)).toBe(false);
    },
  );

  it.each(["/robots.txt/nested", "/robots.txtx", "/sitemap.xml.gz"])(
    "rejects %s — the SEO files are exact matches, not prefixes",
    (p) => {
      expect(isPublic(p)).toBe(false);
    },
  );

  describe("the Slack events delivery path", () => {
    it("is public — Slack carries no session, only a signature", () => {
      expect(isPublic("/api/webhooks/slack")).toBe(true);
    });

    it.each([
      "/api/webhooks",
      "/api/webhooks/slack/extra",
      "/api/webhooks/slacks",
      "/api/webhooks/discord",
    ])(
      "keeps %s protected — the entry is one exact path, not a subtree",
      (p) => {
        expect(isPublic(p)).toBe(false);
      },
    );
  });

  // The OAuth handshake routes are deliberately NOT public: a lapsed session
  // on the vendor's consent page must land on login and come back with the
  // code and state intact, which only the protected-path redirect gives.
  it.each(["/connect/slack/start", "/connect/slack/callback", "/connect/discord/callback"])(
    "treats %s as protected so a lapsed session is sent through login",
    (p) => {
      expect(isPublic(p)).toBe(false);
    },
  );

  describe("the e2e session-injection endpoint", () => {
    it("stays protected unless the test harness flag is set", () => {
      expect(isPublic("/api/test/session")).toBe(false);
    });

    it("opens only with ALLOW_TEST_SESSION=true (route re-gates on prod)", () => {
      process.env.ALLOW_TEST_SESSION = "true";
      try {
        expect(isPublic("/api/test/session")).toBe(true);
        expect(isPublic("/api/test/session/extra")).toBe(false);
        expect(isPublic("/api/test")).toBe(false);
      } finally {
        delete process.env.ALLOW_TEST_SESSION;
      }
    });
  });
});
