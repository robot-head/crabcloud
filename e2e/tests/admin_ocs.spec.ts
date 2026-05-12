import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

test.describe("Admin OCS endpoints", () => {
    // Best-effort cleanup so a partial-failure run doesn't poison the next CI run
    // with 409 conflicts on user/group create.
    test.afterAll(async ({ request }) => {
        try {
            const login = await request.post("/index.php/login", {
                data: { username: "admin", password: "hunter2" },
                headers: { "content-type": "application/json" },
                maxRedirects: 0,
            });
            const setCookie = login.headers()["set-cookie"];
            if (!setCookie) return;
            const m = /oc_sessionPassphrase=([^;\n]+)/.exec(setCookie);
            if (!m) return;
            const cookie = `oc_sessionPassphrase=${m[1]}`;
            await request.delete("/ocs/v2.php/cloud/users/bob?format=json", {
                headers: { "ocs-apirequest": "true", cookie },
            });
            await request.delete("/ocs/v2.php/cloud/groups/qa?format=json", {
                headers: { "ocs-apirequest": "true", cookie },
            });
        } catch {
            // best-effort
        }
    });

    test("admin can create -> get -> edit -> disable -> enable -> delete a user", async ({ request }) => {
        // 1. Login as bootstrap admin.
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        expect(login.status()).toBe(200);
        const cookieHeader = login.headers()["set-cookie"];
        expect(cookieHeader).toBeTruthy();
        const adminMatch = /oc_sessionPassphrase=([^;\n]+)/.exec(cookieHeader!);
        expect(adminMatch).not.toBeNull();
        const sessionValue = adminMatch![1];
        const cookie = `oc_sessionPassphrase=${sessionValue}`;

        // 2. Create bob.
        const create = await request.post("/ocs/v2.php/cloud/users", {
            form: { userid: "bob", password: "bobpw", email: "bob@example.com" },
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(create.status()).toBe(200);
        const createBody = await create.json();
        expect(createBody.ocs.data.id).toBe("bob");

        // 3. GET bob — full record.
        const got = await request.get("/ocs/v2.php/cloud/users/bob?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(got.status()).toBe(200);
        const gotBody = await got.json();
        expect(gotBody.ocs.data.id).toBe("bob");
        expect(gotBody.ocs.data.enabled).toBe(true);
        expect(gotBody.ocs.data.email).toBe("bob@example.com");

        // 4. PUT displayname.
        const editName = await request.put("/ocs/v2.php/cloud/users/bob?format=json", {
            form: { key: "displayname", value: "Bob B." },
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(editName.status()).toBe(200);

        // 5. Confirm via GET.
        const gotAgain = await request.get("/ocs/v2.php/cloud/users/bob?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        const gotAgainBody = await gotAgain.json();
        expect(gotAgainBody.ocs.data["display-name"]).toBe("Bob B.");

        // 6. Login as bob to mint a Bearer token.
        const bobLogin = await request.post("/index.php/login", {
            data: { username: "bob", password: "bobpw" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        expect(bobLogin.status()).toBe(200);
        const bobCookie = bobLogin.headers()["set-cookie"];
        expect(bobCookie).toBeTruthy();
        const bobMatch = /oc_sessionPassphrase=([^;\n]+)/.exec(bobCookie!);
        expect(bobMatch).not.toBeNull();
        const bobSession = bobMatch![1];

        // Get bob's app password via the bridge endpoint.
        const gap = await request.get("/ocs/v2.php/core/getapppassword?format=json", {
            headers: { "ocs-apirequest": "true", cookie: `oc_sessionPassphrase=${bobSession}` },
        });
        expect(gap.status()).toBe(200);
        const bobToken: string = (await gap.json()).ocs.data.apppassword;

        // 7. Bob's Bearer token works pre-disable.
        const meBefore = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: { "ocs-apirequest": "true", authorization: `Bearer ${bobToken}` },
        });
        expect(meBefore.status()).toBe(200);

        // 8. Admin disables bob.
        const disable = await request.put("/ocs/v2.php/cloud/users/bob/disable?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(disable.status()).toBe(200);

        // 9. Bob's token is now 401.
        const meAfter = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: { "ocs-apirequest": "true", authorization: `Bearer ${bobToken}` },
        });
        expect(meAfter.status()).toBe(401);

        // 10. Admin re-enables bob.
        const enable = await request.put("/ocs/v2.php/cloud/users/bob/enable?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(enable.status()).toBe(200);

        // 11. Admin deletes bob.
        const del = await request.delete("/ocs/v2.php/cloud/users/bob?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(del.status()).toBe(200);

        // 12. GET bob → 404.
        const after = await request.get("/ocs/v2.php/cloud/users/bob?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(after.status()).toBe(404);
    });

    test("admin can create and delete a group", async ({ request }) => {
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        const groupCookieHeader = login.headers()["set-cookie"];
        expect(groupCookieHeader).toBeTruthy();
        const groupMatch = /oc_sessionPassphrase=([^;\n]+)/.exec(groupCookieHeader!);
        expect(groupMatch).not.toBeNull();
        const cookie = `oc_sessionPassphrase=${groupMatch![1]}`;

        const create = await request.post("/ocs/v2.php/cloud/groups", {
            form: { groupid: "qa", displayname: "QA Team" },
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(create.status()).toBe(200);

        const members = await request.get("/ocs/v2.php/cloud/groups/qa?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(members.status()).toBe(200);

        const del = await request.delete("/ocs/v2.php/cloud/groups/qa?format=json", {
            headers: { "ocs-apirequest": "true", cookie },
        });
        expect(del.status()).toBe(200);
    });
});
