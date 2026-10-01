import { defineConfig, devices } from "@playwright/test";

const SERVICE_SECRET = "e2e-service-secret-do-not-use-in-production";
// The dev Postgres, as the module roles `make up` gives the server, so the
// suite runs under the same grants and row-level security.
const AUTH_DB = "postgresql://auth:auth_dev@localhost:5432/telmoni";
const NOTIFICATIONS_DB = "postgresql://notifications:notifications_dev@localhost:5432/telmoni";

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
        PORT: "8082",
        SERVICE_SECRET,
        // Never reached: every e2e person is signed in through the test door.
        // The provider's endpoints are discovered on first use, so a dead
        // issuer costs nothing until something signs in through it.
        OIDC_ISSUER: "http://127.0.0.1:1",
        OIDC_CLIENT_ID: "client_e2e_never_called",
        OIDC_CLIENT_SECRET: "e2e_never_called",
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
      },
    },
  ],
});
