import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

test.describe("Crabcloud SSR + hydration", () => {
    test("home page SSRs with hydration marker and hydrates", async ({ page }) => {
        // Capture the response before JS executes to verify the SSR snapshot.
        const response = await page.goto("/");
        expect(response).not.toBeNull();
        expect(response!.status()).toBe(200);

        const htmlBeforeJs = await response!.text();
        expect(htmlBeforeJs).toContain("Welcome, guest");
        expect(htmlBeforeJs).toContain("data-hydrated=\"false\"");
        expect(htmlBeforeJs).toContain("<script id=\"__dx_ctx\"");

        // After the WASM bundle loads + use_effect runs, the marker flips.
        await expect(page.locator("#app-root")).toHaveAttribute(
            "data-hydrated",
            "true",
            { timeout: 10_000 },
        );
    });

    test("login flow then home shows authenticated greeting", async ({ page, request }) => {
        // POST to /index.php/login directly (the form's action). Use the
        // request context so we can capture and replay the cookie.
        const loginResp = await request.post("/index.php/login", {
            form: { username: "admin", password: "hunter2" },
            maxRedirects: 0,
        });
        expect(loginResp.status()).toBe(303);

        const cookie = loginResp.headers()["set-cookie"];
        expect(cookie).toContain("oc_sessionPassphrase=");

        // Visit `/` with the new session cookie.
        const sessionValue = /oc_sessionPassphrase=([^;]+)/.exec(cookie!)![1];
        await page.context().addCookies([{
            name: "oc_sessionPassphrase",
            value: sessionValue,
            url: new URL("/", BASE_URL).toString(),
        }]);

        const homeResp = await page.goto("/");
        expect(homeResp!.status()).toBe(200);
        await expect(page.locator("body")).toContainText("Welcome, admin");

        // And hydration still happens.
        await expect(page.locator("#app-root")).toHaveAttribute(
            "data-hydrated",
            "true",
            { timeout: 10_000 },
        );
    });

    test("404 path returns 404 status", async ({ page }) => {
        const response = await page.goto("/this/does/not/exist");
        expect(response!.status()).toBe(404);
        await expect(page.locator("body")).toContainText("Not Found");
    });
});
