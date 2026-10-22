import { defineConfig, devices } from "@playwright/test";

const SERVICE_SECRET = "e2e-service-secret-do-not-use-in-production";
// The dev Postgres, as the module roles `make up` gives the server, so the
// suite runs under the same grants and row-level security.
const AUTH_DB = "postgresql://auth:auth_dev@localhost:5432/telmoni";
const NOTIFICATIONS_DB = "postgresql://notifications:notifications_dev@localhost:5432/telmoni";
const TELEMETRY_DB = "postgresql://telemetry:telemetry_dev@localhost:5432/telmoni";
// The dev ClickHouse, as the module's own user. No page the suite opens reads
// a span, and the server dials ClickHouse at its first query, never at boot.
const TELEMETRY_CLICKHOUSE = "http://telemetry:telemetry_dev@localhost:8123";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? "github" : "html",
  use: {
    baseURL: "http://localhost:3000",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "mobile-chromium",
      use: { ...devices["iPhone 13"], browserName: "chromium" },
    },
  ],
  webServer: [
    {
      command: "../target/debug/telmoni serve",
      url: "http://localhost:8082/health",
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
      env: {
        AUTH_DATABASE_URL: AUTH_DB,
        NOTIFICATIONS_DATABASE_URL: NOTIFICATIONS_DB,
        TELEMETRY_DATABASE_URL: TELEMETRY_DB,
        TELEMETRY_CLICKHOUSE_URL: TELEMETRY_CLICKHOUSE,
        PORT: "8082",
        SERVICE_SECRET,
        // Dex, the compose file's `oidc` profile, which `make e2e` starts:
        // `oidc.spec.ts` signs in through it, every other spec through the
        // test door. Its endpoints are discovered on first use, so the other
        // specs never reach it.
        AUTH_CUSTOM_ISSUER: "http://127.0.0.1:5556/dex",
        AUTH_CUSTOM_CLIENT_ID: "telmoni-local",
        AUTH_CUSTOM_CLIENT_SECRET: "telmoni-local-dex-secret",
        AUTH_CUSTOM_NAME: "Dex",
        AUTH_DOMAINS_WITH_SSO_ENFORCEMENT: "sso.example",
        // As a deployment starts, whatever .env says: the form on, sign-ups
        // closed. The test door opens accounts either way.
        AUTH_DISABLE_USERNAME_PASSWORD: "false",
        AUTH_DISABLE_SIGNUP: "true",
        APP_URL: "http://localhost:3000",
        // Auth's half of the test door, which mints the sessions the
        // console's half seals. Refused in a pod.
        ALLOW_TEST_SESSION: "true",
      },
    },
    {
      command: "npm run dev",
      url: "http://localhost:3000",
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
      env: {
        ALLOW_TEST_SESSION: "true",
        AUTH_SECRET: "playwright-test-secret-do-not-use-in-production",
        AUTH_URL: "http://localhost:3000",
        SERVICE_SECRET,
        SERVER_URL: "http://localhost:8082",
        REDIS_URL: "redis://localhost:6379",
        // A deployment that published no legal documents, whatever
        // web/.env.local names: `/legal/*` then answers the console's own Page
        // not found, as on a checkout with no env file, rather than a redirect
        // off this machine. Next never lets an env file override a variable
        // the process already has; a console Playwright reuses keeps its own.
        LEGAL_URL: "",
      },
    },
  ],
});
