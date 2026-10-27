import { defineConfig, devices } from "@playwright/test";

// Browser smoke test of the multi-user flow (docs/ROADMAP.md, P1.5 criterion 8). It drives a
// throwaway stack started by scripts/e2e-stack.sh on a test port, never an instance on :8073.
// Locally: scripts/e2e.sh (starts PostgreSQL and NATS in containers). CI: see ci.yml.
const port = Number(process.env.E2E_PORT ?? 18073);
if (port === 8073) throw new Error("E2E_PORT must not be the port of a real instance (8073)");

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.e2e.ts",
  // One shared stack and one database: the tests are a single ordered scenario.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  outputDir: "test-results",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    acceptDownloads: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "../scripts/e2e-stack.sh",
    url: `http://127.0.0.1:${port}/readyz`,
    // Never adopt a server that is already there: the scenario needs an empty database.
    reuseExistingServer: false,
    timeout: 120_000,
    gracefulShutdown: { signal: "SIGTERM", timeout: 10_000 },
    stdout: "pipe",
    stderr: "pipe",
    env: { E2E_PORT: String(port) },
  },
});
