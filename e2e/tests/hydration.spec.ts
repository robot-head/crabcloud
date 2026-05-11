import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

test.describe("Crabcloud SSR + hydration", () => {
    test("home page SSRs with hydration marker and hydrates", async ({ page }) => {
        const response = await page.goto("/");
        expect(response).not.toBeNull();
        expect(response!.status()).toBe(200);

        const htmlBeforeJs = await response!.text();
        expect(htmlBeforeJs).toContain("Welcome, guest");
        expect(htmlBeforeJs).toContain("data-hydrated=\"false\"");
        // CSRF token surfaces as a <meta requesttoken> tag in the document
        // head; the custom __dx_ctx JSON payload is gone — context now rides
        // the standard Dioxus 0.7 hydration data channel via use_server_cached.
        expect(htmlBeforeJs).toContain("name=\"requesttoken\"");

        // After the WASM bundle loads + use_effect runs, the marker flips.
        await expect(page.locator("#app-root")).toHaveAttribute(
            "data-hydrated",
            "true",
            { timeout: 10_000 },
        );
    });

    test("login flow then home shows authenticated greeting", async ({ page, request }) => {
        // /index.php/login is a Dioxus #[server] function (JSON codec, 200 on
        // success). SessionLayer still attaches the session cookie via
        // Set-Cookie regardless of the body shape.
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
    });

    test("404 path returns 404 status with rendered NotFound page", async ({ page }) => {
        const response = await page.goto("/this/does/not/exist");
        expect(response!.status()).toBe(404);
        // Assert against the SSR response body (what crawlers see) rather
        // than the live DOM. The status code is set by the NotFoundRoute
        // component via `FullstackContext::commit_http_status`.
        const html = await response!.text();
        expect(html).toContain("404 — Not Found");
    });
});
