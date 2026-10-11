import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    // Nothing in the SDK touches a DOM: the suite only builds clients.
    environment: "node",
  },
});
