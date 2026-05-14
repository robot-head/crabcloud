import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

async function login(request: any): Promise<string> {
    const r = await request.post("/index.php/login", {
        data: { username: "admin", password: "hunter2" },
        headers: { "content-type": "application/json" },
        maxRedirects: 0,
    });
    expect(r.status()).toBe(200);
    const setCookie = r.headers()["set-cookie"];
    const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie!);
    expect(m).not.toBeNull();
    return `oc_sessionPassphrase=${m![1]}`;
}

async function loginInBrowser(page: any) {
    const cookie = await login(page.request);
    const value = /oc_sessionPassphrase=([^;]+)/.exec(cookie)![1];
    await page.context().addCookies([
        { name: "oc_sessionPassphrase", value, url: BASE_URL },
    ]);
}

// The Files page's listing, toolbar buttons, and row click handlers are all
// owned by the client-side WASM bundle: SSR emits the chrome but `list_dir`
// is only invoked after hydration, and onclick handlers aren't attached
// until then either. Tests that interact with the page must wait for
// `data-hydrated="true"` first (set by `App` in crabcloud-app/src/app.rs).
async function gotoFiles(page: any, path: string = "/apps/files/") {
    const r = await page.goto(path);
    await expect(page.locator("#app-root")).toHaveAttribute(
        "data-hydrated",
        "true",
        { timeout: 10_000 },
    );
    return r;
}

test.describe("Files web UI", () => {
    test("anonymous /apps/files redirects to login with redirect_url", async ({ page }) => {
        const r = await page.goto("/apps/files/", { waitUntil: "domcontentloaded" });
        // Redirect target is the `/login` UI route, not `/index.php/login`
        // (which is the POST-only server-fn endpoint and returns 405 on GET).
        expect(r!.url()).toContain("/login");
        expect(r!.url()).toContain("redirect_url");
        // Server normalizes the root path: `/apps/files/` → `/apps/files`
        // (no trailing slash). The router serves both, so this is fine.
        expect(decodeURIComponent(r!.url())).toContain("/apps/files");
    });

    test("authenticated /apps/files renders chrome + folder list", async ({ page }) => {
        await loginInBrowser(page);
        const r = await gotoFiles(page);
        expect(r!.status()).toBe(200);
        await expect(page.locator(".files-page")).toBeVisible();
        await expect(page.locator(".sidebar-item.active")).toContainText("All files");
    });

    test("mkdir + rename + delete round-trip", async ({ page }) => {
        await loginInBrowser(page);
        await gotoFiles(page);

        // mkdir
        await page.click(".files-tb-primary");
        await page.fill(".files-mkdir-input", "e2e-folder");
        await page.keyboard.press("Enter");
        await expect(page.locator(".files-name", { hasText: "e2e-folder" })).toBeVisible();

        // rename via row ⋯ menu
        await page.click('.files-row:has-text("e2e-folder") .files-overflow-btn');
        await page.click('.files-overflow-item:has-text("Rename")');
        await page.fill(".files-rename-input", "renamed");
        await page.keyboard.press("Enter");
        await expect(page.locator(".files-name", { hasText: "renamed" })).toBeVisible();

        // delete via row ⋯ menu
        await page.click('.files-row:has-text("renamed") .files-overflow-btn');
        await page.click('.files-overflow-item:has-text("Delete")');
        await page.click(".files-modal-confirm");
        await expect(page.locator(".files-name", { hasText: "renamed" })).toHaveCount(0);
    });

    test("listing shows file uploaded via WebDAV", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        // Pre-cleanup: a prior failed run may leave the file behind, in which
        // case PUT returns 204 instead of 201 and the assertion below trips.
        await request.fetch("/dav/files/admin/e2e-upload.txt", { method: "DELETE", headers: { cookie } });
        const r = await request.fetch("/dav/files/admin/e2e-upload.txt", {
            method: "PUT",
            headers: { cookie },
            data: "hello e2e",
        });
        expect(r.status()).toBe(201);
        await gotoFiles(page);
        await expect(page.locator(".files-name", { hasText: "e2e-upload.txt" })).toBeVisible();
        await request.fetch("/dav/files/admin/e2e-upload.txt", { method: "DELETE", headers: { cookie } });
    });

    test("clicking a folder updates the URL", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch("/dav/files/admin/clickable/", { method: "MKCOL", headers: { cookie } });
        await gotoFiles(page);
        await page.click('.files-name-folder:has-text("clickable")');
        await expect(page).toHaveURL(/\/apps\/files\/clickable$/);
        await request.fetch("/dav/files/admin/clickable", { method: "DELETE", headers: { cookie } });
    });

    test("multi-select + cut/paste moves files across folders", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch("/dav/files/admin/src-file.txt", {
            method: "PUT", headers: { cookie }, data: "x",
        });
        await request.fetch("/dav/files/admin/dest-dir/", {
            method: "MKCOL", headers: { cookie },
        });
        await gotoFiles(page);
        await page.click('.files-row:has-text("src-file.txt") input[type=checkbox]');
        await page.click('.files-chip-selection .files-chip-action:has-text("Cut")');
        await page.click('.files-name-folder:has-text("dest-dir")');
        await page.click('.files-chip-clipboard .files-chip-action:has-text("Paste here")');
        await expect(page.locator(".files-name", { hasText: "src-file.txt" })).toBeVisible();
        // Cleanup
        await request.fetch("/dav/files/admin/dest-dir/src-file.txt", { method: "DELETE", headers: { cookie } });
        await request.fetch("/dav/files/admin/dest-dir", { method: "DELETE", headers: { cookie } });
    });

    test("reload preserves folder", async ({ page, request }) => {
        await loginInBrowser(page);
        const cookie = await login(request);
        await request.fetch("/dav/files/admin/persist-dir/", { method: "MKCOL", headers: { cookie } });
        await gotoFiles(page, "/apps/files/persist-dir");
        await page.reload();
        await expect(page).toHaveURL(/\/apps\/files\/persist-dir$/);
        await request.fetch("/dav/files/admin/persist-dir", { method: "DELETE", headers: { cookie } });
    });
});
