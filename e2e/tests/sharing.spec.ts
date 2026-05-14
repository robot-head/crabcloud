import { test, expect, request as pwRequest } from "@playwright/test";

// SP7 batch F: end-to-end sharing scenarios across two browser contexts
// (alice as owner, bob as recipient). All three tests drive the OCS
// sharing API for the *creation* side via `page.request.post(...)` — the
// modal UI is tested in unit-test land and on the screenshots flow; here
// the value is exercising the full router stack + observing recipient-
// side reactivity (bob's listing reflects the share, then loses it on
// revoke).
//
// Auth: `oc_sessionPassphrase` cookie. POST `/index.php/login` returns
// 200 + a Set-Cookie on success. Same pattern as `files.spec.ts` /
// `webdav.spec.ts`.

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";
const OCS_BASE = "/ocs/v2.php/apps/files_sharing/api/v1";

interface LoginResult {
    cookie: string; // serialized "oc_sessionPassphrase=…"
    value: string;  // raw value (no name=)
}

async function loginCookie(request: any, uid: string, password: string): Promise<LoginResult> {
    const r = await request.post("/index.php/login", {
        data: { username: uid, password },
        headers: { "content-type": "application/json" },
        maxRedirects: 0,
    });
    expect(
        r.status(),
        `login as ${uid} failed (status ${r.status()})`,
    ).toBe(200);
    const setCookie = r.headers()["set-cookie"];
    expect(setCookie, `no set-cookie header from /index.php/login for ${uid}`).toBeTruthy();
    const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie!);
    expect(m).not.toBeNull();
    return {
        cookie: `oc_sessionPassphrase=${m![1]}`,
        value: m![1],
    };
}

async function loginInBrowser(page: any, uid: string, password: string): Promise<string> {
    const c = await loginCookie(page.request, uid, password);
    await page.context().addCookies([
        { name: "oc_sessionPassphrase", value: c.value, url: BASE_URL },
    ]);
    return c.cookie;
}

async function gotoFiles(page: any, path: string = "/apps/files/") {
    await page.goto(path);
    await expect(page.locator("#app-root")).toHaveAttribute("data-hydrated", "true", {
        timeout: 10_000,
    });
}

// MKCOL via WebDAV — same pattern as the existing files / webdav specs.
async function mkcol(request: any, cookie: string, uid: string, path: string) {
    const r = await request.fetch(`/dav/files/${uid}${path}`, {
        method: "MKCOL",
        headers: { cookie },
    });
    expect([201, 405]).toContain(r.status()); // 405 = already exists; idempotent
}

// PUT via WebDAV.
async function davPut(
    request: any,
    cookie: string,
    uid: string,
    path: string,
    body: string,
): Promise<number> {
    const r = await request.fetch(`/dav/files/${uid}${path}`, {
        method: "PUT",
        headers: { cookie },
        data: body,
    });
    return r.status();
}

async function davDelete(request: any, cookie: string, uid: string, path: string) {
    await request
        .fetch(`/dav/files/${uid}${path}`, { method: "DELETE", headers: { cookie } })
        .catch(() => {});
}

interface OcsShare {
    id: string;
}

// POST an OCS share. `permissions` defaults to 3 (read + update).
async function createShare(
    request: any,
    cookie: string,
    path: string,
    shareWith: string,
    permissions: number = 3,
): Promise<OcsShare> {
    const r = await request.post(`${OCS_BASE}/shares?format=json`, {
        headers: {
            cookie,
            "ocs-apirequest": "true",
            "content-type": "application/x-www-form-urlencoded",
        },
        data: `path=${encodeURIComponent(path)}&shareType=0&shareWith=${shareWith}&permissions=${permissions}`,
    });
    expect(
        r.status(),
        `POST /shares (${path} → ${shareWith}) returned ${r.status()}: ${await r.text()}`,
    ).toBe(200);
    const body = await r.json();
    const id = body?.ocs?.data?.id;
    expect(id, `OCS response missing id: ${JSON.stringify(body)}`).toBeTruthy();
    return { id: String(id) };
}

async function deleteShare(request: any, cookie: string, id: string) {
    const r = await request.fetch(`${OCS_BASE}/shares/${id}?format=json`, {
        method: "DELETE",
        headers: { cookie, "ocs-apirequest": "true" },
    });
    expect(r.status()).toBe(200);
}

// Best-effort cleanup of alice's outgoing shares + her home folders.
async function reset(request: any, cookie: string, uid: string) {
    // Tear down any leftover shares first so the next test's recipient
    // doesn't see stale mounts.
    try {
        const r = await request.fetch(`${OCS_BASE}/shares?format=json`, {
            method: "GET",
            headers: { cookie, "ocs-apirequest": "true" },
        });
        if (r.ok()) {
            const body = await r.json();
            const data = body?.ocs?.data;
            if (Array.isArray(data)) {
                for (const row of data) {
                    if (row?.id) await deleteShare(request, cookie, String(row.id));
                }
            }
        }
    } catch {
        // best-effort
    }
    // Nuke the test folders. Use specific names rather than a `list`-
    // and-delete-everything approach so we don't fight other tests'
    // fixtures if they share the same admin user (we don't here, but the
    // explicit list is safer).
    for (const name of ["SharedFolder", "ReadOnlyShare", "RevokeShare"]) {
        await davDelete(request, cookie, uid, `/${name}`);
    }
}

test.describe("Sharing", () => {
    test.beforeEach(async () => {
        // Each test resets both alice's and bob's state through its own
        // request context; nothing global to do here.
    });

    test("alice shares a folder with bob; bob sees it at root with the badge", async ({
        browser,
    }) => {
        const aliceCtx = await browser.newContext();
        const bobCtx = await browser.newContext();
        try {
            const aliceLogin = await loginCookie(aliceCtx.request, "alice", "hunter2");
            const bobLogin = await loginCookie(bobCtx.request, "bob", "hunter2");

            await reset(aliceCtx.request, aliceLogin.cookie, "alice");

            // Alice creates `/SharedFolder` via WebDAV (drives the same
            // storage stack the UI would use; pre-share content is
            // optional for the visibility assertion).
            await mkcol(aliceCtx.request, aliceLogin.cookie, "alice", "/SharedFolder");

            // Create the share. Permissions = 3 (read | update).
            await createShare(
                aliceCtx.request,
                aliceLogin.cookie,
                "/SharedFolder",
                "bob",
                3,
            );

            // Bob logs in and lists his root. The share mount surfaces
            // a `SharedFolder` row decorated with `(shared by alice)`.
            const bobPage = await bobCtx.newPage();
            await bobPage.context().addCookies([
                {
                    name: "oc_sessionPassphrase",
                    value: bobLogin.value,
                    url: BASE_URL,
                },
            ]);
            await gotoFiles(bobPage);
            await expect(
                bobPage.locator('.files-row:has-text("SharedFolder")'),
            ).toBeVisible({ timeout: 10_000 });
            await expect(
                bobPage.locator('.files-row:has-text("SharedFolder") .row-shared-by'),
            ).toContainText("shared by alice");

            // Cleanup.
            await reset(aliceCtx.request, aliceLogin.cookie, "alice");
        } finally {
            await aliceCtx.close();
            await bobCtx.close();
        }
    });

    test("bob cannot upload to a read-only share", async ({ browser }) => {
        const aliceCtx = await browser.newContext();
        const bobCtx = await browser.newContext();
        try {
            const aliceLogin = await loginCookie(aliceCtx.request, "alice", "hunter2");
            const bobLogin = await loginCookie(bobCtx.request, "bob", "hunter2");

            await reset(aliceCtx.request, aliceLogin.cookie, "alice");

            await mkcol(aliceCtx.request, aliceLogin.cookie, "alice", "/ReadOnlyShare");

            // permissions=1 → read-only.
            await createShare(
                aliceCtx.request,
                aliceLogin.cookie,
                "/ReadOnlyShare",
                "bob",
                1,
            );

            // Bob attempts to upload via WebDAV through his own mount —
            // the share is surfaced at `/dav/files/bob/ReadOnlyShare`.
            // SharedSubrootStorage::write returns PermissionDenied; the
            // DAV layer maps that to 403.
            const status = await davPut(
                bobCtx.request,
                bobLogin.cookie,
                "bob",
                "/ReadOnlyShare/forbidden.txt",
                "should be rejected",
            );
            expect(status).toBe(403);

            await reset(aliceCtx.request, aliceLogin.cookie, "alice");
        } finally {
            await aliceCtx.close();
            await bobCtx.close();
        }
    });

    test("alice revokes; bob's share-mount disappears after reload", async ({
        browser,
    }) => {
        const aliceCtx = await browser.newContext();
        const bobCtx = await browser.newContext();
        try {
            const aliceLogin = await loginCookie(aliceCtx.request, "alice", "hunter2");
            const bobLogin = await loginCookie(bobCtx.request, "bob", "hunter2");

            await reset(aliceCtx.request, aliceLogin.cookie, "alice");

            await mkcol(aliceCtx.request, aliceLogin.cookie, "alice", "/RevokeShare");
            const share = await createShare(
                aliceCtx.request,
                aliceLogin.cookie,
                "/RevokeShare",
                "bob",
                3,
            );

            const bobPage = await bobCtx.newPage();
            await bobPage.context().addCookies([
                {
                    name: "oc_sessionPassphrase",
                    value: bobLogin.value,
                    url: BASE_URL,
                },
            ]);
            await gotoFiles(bobPage);
            await expect(
                bobPage.locator('.files-row:has-text("RevokeShare")'),
            ).toBeVisible({ timeout: 10_000 });

            // Alice revokes.
            await deleteShare(aliceCtx.request, aliceLogin.cookie, share.id);

            // Bob reloads — the row vanishes because the share mount is
            // no longer surfaced by `ShareMountResolver`.
            await bobPage.reload();
            await expect(bobPage.locator("#app-root")).toHaveAttribute(
                "data-hydrated",
                "true",
                { timeout: 10_000 },
            );
            await expect(
                bobPage.locator('.files-row:has-text("RevokeShare")'),
            ).toHaveCount(0, { timeout: 10_000 });

            await reset(aliceCtx.request, aliceLogin.cookie, "alice");
        } finally {
            await aliceCtx.close();
            await bobCtx.close();
        }
    });
});

// Anchor the unused `pwRequest` import for type-only consumers above.
// (Removed at minify; here only to keep TS strict happy if we extend.)
void pwRequest;
void loginInBrowser;
