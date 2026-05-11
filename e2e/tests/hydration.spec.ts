import { test, expect, type Page } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

// Capture browser console + page errors so they appear in CI annotations
// when a hydration assertion times out (otherwise the failure is opaque).
function watchPage(page: Page): { dump: () => void } {
    const msgs: string[] = [];
    page.on("console", (m) => {
        msgs.push(`[console.${m.type()}] ${m.text()}`);
    });
    page.on("pageerror", (e) => {
        msgs.push(`[pageerror] ${e.name}: ${e.message}\n${e.stack ?? ""}`);
    });
    page.on("requestfailed", (req) => {
        msgs.push(`[requestfailed] ${req.method()} ${req.url()} - ${req.failure()?.errorText ?? "unknown"}`);
    });
    page.on("response", (res) => {
        if (res.status() >= 400) {
            msgs.push(`[response>=400] ${res.status()} ${res.url()}`);
        }
    });
    return {
        dump() {
            for (const m of msgs) {
                console.log(`::error title=Browser event::${m.replace(/\n/g, " | ")}`);
            }
        },
    };
}

test.describe("Crabcloud SSR + hydration", () => {
    test("home page SSRs with hydration marker and hydrates", async ({ page }) => {
        const watcher = watchPage(page);
        try {
            const response = await page.goto("/");
            expect(response).not.toBeNull();
            expect(response!.status()).toBe(200);

            const htmlBeforeJs = await response!.text();
            expect(htmlBeforeJs).toContain("Welcome, guest");
            expect(htmlBeforeJs).toContain("data-hydrated=\"false\"");
            expect(htmlBeforeJs).toContain("name=\"requesttoken\"");

            await expect(page.locator("#app-root")).toHaveAttribute(
                "data-hydrated",
                "true",
                { timeout: 10_000 },
            );
        } finally {
            watcher.dump();
        }
    });

    test("login flow then home shows authenticated greeting", async ({ page, request }) => {
        const watcher = watchPage(page);
        try {
            const loginResp = await request.post("/index.php/login", {
                data: { username: "admin", password: "hunter2" },
                headers: { "content-type": "application/json" },
            });
            expect(loginResp.status()).toBe(200);

            const cookie = loginResp.headers()["set-cookie"];
            expect(cookie).toContain("oc_sessionPassphrase=");

            const sessionValue = /oc_sessionPassphrase=([^;]+)/.exec(cookie!)![1];
            await page.context().addCookies([{
                name: "oc_sessionPassphrase",
                value: sessionValue,
                url: new URL("/", BASE_URL).toString(),
            }]);

            const homeResp = await page.goto("/");
            expect(homeResp!.status()).toBe(200);
            await expect(page.locator("body")).toContainText("Welcome, admin");

            await expect(page.locator("#app-root")).toHaveAttribute(
                "data-hydrated",
                "true",
                { timeout: 10_000 },
            );
        } finally {
            watcher.dump();
        }
    });

    test("404 path returns 404 status with rendered NotFound page", async ({ page }) => {
        const response = await page.goto("/this/does/not/exist");
        expect(response!.status()).toBe(404);
        const html = await response!.text();
        expect(html).toContain("404 — Not Found");
    });
});
