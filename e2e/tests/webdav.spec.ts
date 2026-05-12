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

    test("chunked upload: MKCOL + PUT chunks + MOVE commit", async ({ request }) => {
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        const setCookie = login.headers()["set-cookie"]!;
        const cookie = `oc_sessionPassphrase=${/oc_sessionPassphrase=([^;\n]+)/.exec(setCookie)![1]}`;

        const uploadId = `chunk-test-${Date.now()}`;
        const destinationPath = `/dav/files/admin/chunked-${Date.now()}.txt`;
        const destinationUrl = `${process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765"}${destinationPath}`;

        // 1. MKCOL begins the upload.
        const begin = await request.fetch(`/dav/uploads/admin/${uploadId}`, {
            method: "MKCOL",
            headers: { cookie, destination: destinationUrl },
        });
        expect(begin.status()).toBe(201);

        // 2. PUT two chunks. Each PUT returns ETag = sha256(chunk_bytes) hex.
        const chunks: string[] = ["AAA", "BBB"];
        const tags: { part_number: number; etag: string }[] = [];
        for (let i = 0; i < chunks.length; i++) {
            const r = await request.fetch(`/dav/uploads/admin/${uploadId}/${i + 1}`, {
                method: "PUT",
                headers: { cookie },
                data: chunks[i],
            });
            expect(r.status()).toBe(201);
            const etag = r.headers()["etag"]!;
            // ETag returned by put_part is the raw sha256 hex (no quotes per Batch G).
            tags.push({ part_number: i + 1, etag });
        }

        // 3. MOVE commits the upload.
        const commit = await request.fetch(`/dav/uploads/admin/${uploadId}/.file`, {
            method: "MOVE",
            headers: {
                cookie,
                destination: destinationUrl,
                "x-crabcloud-part-tags": JSON.stringify(tags),
            },
        });
        expect(commit.status()).toBe(201);

        // 4. GET the assembled file — should be the concatenation.
        const get = await request.fetch(destinationPath, {
            method: "GET",
            headers: { cookie },
        });
        expect(get.status()).toBe(200);
        expect(await get.text()).toBe("AAABBB");

        // 5. Cleanup.
        await request.fetch(destinationPath, { method: "DELETE", headers: { cookie } });
    });

    test("LOCK acquires token, UNLOCK releases", async ({ request }) => {
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        const setCookie = login.headers()["set-cookie"]!;
        const cookie = `oc_sessionPassphrase=${/oc_sessionPassphrase=([^;\n]+)/.exec(setCookie)![1]}`;

        // Seed a file to lock.
        const path = `/dav/files/admin/locked-${Date.now()}.txt`;
        await request.fetch(path, { method: "PUT", headers: { cookie }, data: "x" });

        // LOCK with depth 0 + 60s timeout + a minimal lockinfo body.
        const lockBody = `<?xml version="1.0"?>
<d:lockinfo xmlns:d="DAV:">
  <d:lockscope><d:exclusive/></d:lockscope>
  <d:locktype><d:write/></d:locktype>
  <d:owner>e2e-test</d:owner>
</d:lockinfo>`;
        const lock = await request.fetch(path, {
            method: "LOCK",
            headers: {
                cookie,
                depth: "0",
                timeout: "Second-60",
                "content-type": "application/xml",
            },
            data: lockBody,
        });
        expect(lock.status()).toBe(200);
        const lockToken = lock.headers()["lock-token"];
        expect(lockToken).toBeTruthy();
        // Lock-Token header is in the form <urn:uuid:...>.
        const tokenInner = /^<(.+)>$/.exec(lockToken!.trim())?.[1];
        expect(tokenInner).toBeTruthy();

        // PUT without If: header should now return 423 Locked.
        const blocked = await request.fetch(path, {
            method: "PUT",
            headers: { cookie },
            data: "blocked",
        });
        expect(blocked.status()).toBe(423);

        // UNLOCK with correct token returns 204.
        const unlock = await request.fetch(path, {
            method: "UNLOCK",
            headers: { cookie, "lock-token": lockToken! },
        });
        expect(unlock.status()).toBe(204);

        // After UNLOCK, PUT succeeds again.
        const post = await request.fetch(path, {
            method: "PUT",
            headers: { cookie },
            data: "after",
        });
        expect(post.status()).toBe(204);

        // Cleanup.
        await request.fetch(path, { method: "DELETE", headers: { cookie } });
    });
});
