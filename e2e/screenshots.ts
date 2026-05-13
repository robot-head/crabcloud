// One-off screenshot capture for the Files web UI. Not part of the regular
// test suite — invoked via `npm run screenshots` (which loads
// `playwright.screenshots.config.ts`) against a running server at
// CRABCLOUD_E2E_URL (default 127.0.0.1:18765).
//
// Captures four PNGs into docs/screenshots/:
//   files-empty.png        — empty home folder
//   files-list.png         — folder with a few items
//   files-selection.png    — multi-select chip visible
//   files-delete-modal.png — delete confirmation modal
//
// Pre-populates a few dummy files/folders via WebDAV PUT/MKCOL before each
// shot, then deletes them on teardown.

import { test, expect } from "@playwright/test";
import * as path from "path";
import * as fs from "fs";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";
const OUT_DIR = path.resolve(__dirname, "..", "docs", "screenshots");

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

async function setBrowserCookie(page: any) {
    const cookie = await login(page.request);
    const value = /oc_sessionPassphrase=([^;]+)/.exec(cookie)![1];
    await page.context().addCookies([
        { name: "oc_sessionPassphrase", value, url: BASE_URL },
    ]);
    return cookie;
}

// Navigate to a Files page and wait for the WASM bundle to hydrate. Without
// this the list never renders (list_dir is a client-side server-fn call) and
// onclick handlers aren't attached, so clicking checkboxes / overflow buttons
// is a no-op. Same pattern as tests/files.spec.ts.
async function gotoFiles(page: any, path: string = "/apps/files/") {
    await page.goto(path);
    await expect(page.locator("#app-root")).toHaveAttribute(
        "data-hydrated",
        "true",
        { timeout: 10_000 },
    );
}

async function cleanHome(request: any, cookie: string) {
    // Best-effort: list home and DELETE each top-level entry.
    const r = await request.post("/api/files/list", {
        headers: { cookie, "content-type": "application/json", "ocs-apirequest": "true" },
        data: { path: "/" },
    });
    if (!r.ok()) return;
    let entries: any[] = [];
    try {
        entries = JSON.parse(await r.text());
    } catch {
        return;
    }
    for (const e of entries) {
        await request.fetch(`/dav/files/admin${e.path}`, {
            method: "DELETE",
            headers: { cookie },
        }).catch(() => {});
    }
}

async function seed(request: any, cookie: string) {
    await request.fetch("/dav/files/admin/photos/", { method: "MKCOL", headers: { cookie } });
    await request.fetch("/dav/files/admin/documents/", { method: "MKCOL", headers: { cookie } });
    await request.fetch("/dav/files/admin/work/", { method: "MKCOL", headers: { cookie } });
    await request.fetch("/dav/files/admin/notes.txt", {
        method: "PUT", headers: { cookie }, data: "Buy milk. Walk dog.",
    });
    await request.fetch("/dav/files/admin/itinerary.pdf", {
        method: "PUT", headers: { cookie }, data: "fake pdf bytes",
    });
    await request.fetch("/dav/files/admin/screenshot.png", {
        method: "PUT", headers: { cookie }, data: "fake png bytes",
    });
}

test.beforeAll(() => {
    if (!fs.existsSync(OUT_DIR)) fs.mkdirSync(OUT_DIR, { recursive: true });
});

test.use({ viewport: { width: 1200, height: 720 } });

test("empty home", async ({ page, request }) => {
    const cookie = await setBrowserCookie(page);
    await cleanHome(request, cookie);
    await gotoFiles(page);
    await expect(page.locator(".files-empty")).toBeVisible();
    await page.screenshot({ path: path.join(OUT_DIR, "files-empty.png"), fullPage: false });
});

test("populated list", async ({ page, request }) => {
    const cookie = await setBrowserCookie(page);
    await cleanHome(request, cookie);
    await seed(request, cookie);
    await gotoFiles(page);
    await expect(page.locator(".files-name", { hasText: "notes.txt" })).toBeVisible();
    await page.screenshot({ path: path.join(OUT_DIR, "files-list.png"), fullPage: false });
});

test("multi-select chip", async ({ page, request }) => {
    const cookie = await setBrowserCookie(page);
    await cleanHome(request, cookie);
    await seed(request, cookie);
    await gotoFiles(page);
    await page.click('.files-row:has-text("notes.txt") input[type=checkbox]');
    await page.click('.files-row:has-text("itinerary.pdf") input[type=checkbox]');
    await expect(page.locator(".files-chip-selection")).toBeVisible();
    await page.screenshot({ path: path.join(OUT_DIR, "files-selection.png"), fullPage: false });
});

test("delete modal", async ({ page, request }) => {
    const cookie = await setBrowserCookie(page);
    await cleanHome(request, cookie);
    await seed(request, cookie);
    await gotoFiles(page);
    await page.click('.files-row:has-text("notes.txt") .files-overflow-btn');
    await page.click('.files-overflow-item:has-text("Delete")');
    await expect(page.locator(".files-modal")).toBeVisible();
    await page.screenshot({ path: path.join(OUT_DIR, "files-delete-modal.png"), fullPage: false });
    await page.click(".files-modal-cancel"); // dismiss so cleanup hooks find a clean state
});

test.afterAll(async ({ request }) => {
    try {
        const cookie = await login(request);
        await cleanHome(request, cookie);
    } catch {}
});
