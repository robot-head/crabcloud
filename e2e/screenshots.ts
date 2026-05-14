// One-off screenshot capture for the Files web UI. Not part of the regular
// test suite — invoked via `npm run screenshots` (which loads
// `playwright.screenshots.config.ts`) against a running server at
// CRABCLOUD_E2E_URL (default 127.0.0.1:18765).
//
// Captures five PNGs into docs/screenshots/:
//   files-empty.png        — empty home folder
//   files-list.png         — folder with a few items
//   files-selection.png    — multi-select chip visible
//   files-delete-modal.png — delete confirmation modal
//   share-modal.png        — share modal with recipient autocomplete open
//
// Pre-populates a few dummy files/folders via WebDAV PUT/MKCOL before each
// shot, then deletes them on teardown. The share-modal shot also needs a
// real recipient candidate to surface in the autocomplete — see the
// per-test setup at the bottom of this file.

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

// Share modal — opens from the ⋯ menu of a folder row, then a recipient
// is typed to surface the autocomplete dropdown. The screenshots fixture
// only seeds `admin` via `bootstrap_admin`, so the autocomplete needs at
// least one matching candidate. We mint `bob` via the OCS provisioning
// API (admin can create users on the screenshots config) before opening
// the modal. The fixture itself isn't part of the regular e2e suite so
// the extra user is harmless.
test("share modal", async ({ page, request }) => {
    const cookie = await setBrowserCookie(page);
    await cleanHome(request, cookie);
    await seed(request, cookie);

    // Best-effort: provision `bob` so the autocomplete has a hit on
    // "bo". The OCS users endpoint is admin-only and returns 200 on
    // success / 102-ish-or-409 if the user already exists; we don't
    // assert because either outcome is fine for the screenshot.
    await request
        .post("/ocs/v2.php/cloud/users?format=json", {
            headers: {
                cookie,
                "ocs-apirequest": "true",
                "content-type": "application/x-www-form-urlencoded",
            },
            data: "userid=bob&password=hunter2",
        })
        .catch(() => {});

    await gotoFiles(page);
    // Open the ⋯ menu on a folder row and click Share. The Share item
    // renders as "🔗  Share"; match the trailing word to avoid colliding
    // with the chip-bar's "Cut/Paste/etc" actions (which sit elsewhere
    // in the DOM but Playwright's `:has-text` is substring-y).
    await page.click('.files-row:has-text("photos") .files-overflow-btn');
    // Wait for the menu to be open before clicking inside it.
    await expect(page.locator(".files-overflow-menu")).toBeVisible();
    await page.locator(".files-overflow-item", { hasText: "Share" }).click();
    await expect(page.locator(".share-modal")).toBeVisible({ timeout: 5_000 });

    // Type "bo" → triggers the 250ms-debounced autocomplete.
    await page.fill(".share-modal-recipient-input", "bo");
    // Wait for the candidates list to materialize. `share_recipient_search`
    // runs server-side, returns up to 10 hits; the list element renders
    // only when `candidates_now` is non-empty so its presence implies
    // the autocomplete fired.
    await expect(page.locator(".share-modal-candidates")).toBeVisible({
        timeout: 5_000,
    });

    await page.screenshot({
        path: path.join(OUT_DIR, "share-modal.png"),
        fullPage: false,
    });

    // Dismiss so the cleanup hook finds a clean state.
    await page.click(".share-modal-close-btn");
});

test.afterAll(async ({ request }) => {
    try {
        const cookie = await login(request);
        await cleanHome(request, cookie);
    } catch {}
});
