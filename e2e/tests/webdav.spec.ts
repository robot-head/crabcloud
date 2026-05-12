import { test, expect } from "@playwright/test";

// WebDAV files API end-to-end. Drives the live server's `/dav/...` surface
// the same way Nextcloud's desktop/mobile clients would (cookie session +
// raw HTTP methods including OPTIONS / PUT / GET / DELETE / PROPFIND).

async function loginCookie(request: any): Promise<string> {
    const login = await request.post("/index.php/login", {
        data: { username: "admin", password: "hunter2" },
        headers: { "content-type": "application/json" },
        maxRedirects: 0,
    });
    expect(login.status()).toBe(200);
    const setCookie = login.headers()["set-cookie"];
    expect(setCookie).toBeTruthy();
    const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie!);
    expect(m).not.toBeNull();
    return `oc_sessionPassphrase=${m![1]}`;
}

test.describe("WebDAV files API", () => {
    test.afterAll(async ({ request }) => {
        try {
            const cookie = await loginCookie(request);
            await request.fetch("/dav/files/admin/webdav-test.txt", {
                method: "DELETE",
                headers: { cookie },
            });
        } catch {
            // best-effort cleanup
        }
    });

    test("OPTIONS advertises DAV class", async ({ request }) => {
        const cookie = await loginCookie(request);
        const r = await request.fetch("/dav/files/admin", {
            method: "OPTIONS",
            headers: { cookie },
        });
        expect(r.status()).toBe(200);
        expect(r.headers()["dav"]).toContain("1");
    });

    test("PUT then GET then DELETE round-trip", async ({ request }) => {
        const cookie = await loginCookie(request);

        const put = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "PUT",
            headers: { cookie },
            data: "hello world",
        });
        expect(put.status()).toBe(201);

        const get = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "GET",
            headers: { cookie },
        });
        expect(get.status()).toBe(200);
        expect(await get.text()).toBe("hello world");

        const del = await request.fetch("/dav/files/admin/webdav-test.txt", {
            method: "DELETE",
            headers: { cookie },
        });
        expect(del.status()).toBe(204);
    });

    test("PROPFIND Depth:0 returns 207", async ({ request }) => {
        const cookie = await loginCookie(request);

        const r = await request.fetch("/dav/files/admin", {
            method: "PROPFIND",
            headers: { cookie, depth: "0" },
        });
        expect(r.status()).toBe(207);
        const body = await r.text();
        expect(body).toContain("<d:multistatus");
        expect(body).toContain("<d:getetag>");
        expect(body).toContain("<oc:permissions>");
    });
});
