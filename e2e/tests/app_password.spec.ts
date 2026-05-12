import { test, expect } from "@playwright/test";

const BASE_URL = process.env.CRABCLOUD_E2E_URL ?? "http://127.0.0.1:18765";

test.describe("App passwords end-to-end", () => {
    test("login -> getapppassword -> use via Basic + Bearer -> revoke -> 401", async ({ request }) => {
        // 1. Log in via /index.php/login (the bootstrap_admin fixture from
        // e2e.toml makes admin/hunter2 valid).
        const login = await request.post("/index.php/login", {
            data: { username: "admin", password: "hunter2" },
            headers: { "content-type": "application/json" },
            maxRedirects: 0,
        });
        expect(login.status()).toBe(200);
        const cookieHeader = login.headers()["set-cookie"];
        expect(cookieHeader).toBeTruthy();
        const sessionValue = /oc_sessionPassphrase=([^;]+)/.exec(cookieHeader!)![1];

        // 2. Mint a bridge app password via getapppassword (session-only).
        const gap = await request.get("/ocs/v2.php/core/getapppassword?format=json", {
            headers: {
                "ocs-apirequest": "true",
                cookie: `oc_sessionPassphrase=${sessionValue}`,
            },
        });
        expect(gap.status()).toBe(200);
        const gapBody = await gap.json();
        const appPassword: string = gapBody.ocs.data.apppassword;
        expect(appPassword.length).toBeGreaterThan(50);

        // 3. Use it via Basic.
        const me = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: {
                "ocs-apirequest": "true",
                authorization: `Basic ${Buffer.from(`admin:${appPassword}`).toString("base64")}`,
            },
        });
        expect(me.status()).toBe(200);
        const meBody = await me.json();
        expect(meBody.ocs.data.id).toBe("admin");

        // 4. Use it via Bearer.
        const me2 = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: {
                "ocs-apirequest": "true",
                authorization: `Bearer ${appPassword}`,
            },
        });
        expect(me2.status()).toBe(200);

        // 5. Self-revoke via DELETE apppassword (using the token itself).
        const del = await request.delete("/ocs/v2.php/core/apppassword?format=json", {
            headers: {
                "ocs-apirequest": "true",
                authorization: `Bearer ${appPassword}`,
            },
        });
        expect(del.status()).toBe(200);

        // 6. After revoke, the Bearer is 401.
        const after = await request.get("/ocs/v2.php/cloud/user?format=json", {
            headers: {
                "ocs-apirequest": "true",
                authorization: `Bearer ${appPassword}`,
            },
        });
        expect(after.status()).toBe(401);
    });
});
