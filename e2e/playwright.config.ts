import { defineConfig, devices } from '@playwright/test';

// The suite runs against an ALREADY-RUNNING dev environment — runbook step 2
// (`services.sh restart-server` / `restart`) starts server (:3000) and web
// (:8080), and step 3 runs these tests against it. Point elsewhere (e.g.
// staging on :8081) with PLAYWRIGHT_BASE_URL. There is deliberately no
// `webServer:` block: this suite does not own the app's lifecycle.
const baseURL = process.env.PLAYWRIGHT_BASE_URL || 'http://localhost:8080';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: [['list'], ['html', { open: 'never' }]],
  // Best-effort removal of the e2e-* test users/games this run created.
  globalTeardown: './global-teardown.ts',
  use: {
    baseURL,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    // The wasm client takes a moment to boot and hydrate on first load.
    actionTimeout: 15_000,
    navigationTimeout: 30_000,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
