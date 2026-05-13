import { defineConfig, devices } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

// Separate config so the screenshots run is opt-in (`npm run screenshots`)
// and isn't picked up by `npm test`. The default config (`playwright.config.ts`)
// scans `./tests/`; here we point at the single screenshots.ts in this dir.
export default defineConfig({
    testDir: ".",
    testMatch: "screenshots.ts",
    timeout: 60_000,
    expect: { timeout: 10_000 },
    fullyParallel: false,
    retries: 0,
    workers: 1,
    reporter: "list",
    use: {
        baseURL: BASE_URL,
        viewport: { width: 1200, height: 720 },
    },
    projects: [
        {
            name: "chromium",
            use: { ...devices["Desktop Chrome"] },
        },
    ],
});
