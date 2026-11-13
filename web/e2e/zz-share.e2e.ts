import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of public share links (ADR-0029). An admin makes a link; a visitor with a
// fresh browser (no login, no cookies) opens /r/<token>, sees the station and the waterfall
// stream and gets a play button that has not started anything. The embed page may be framed only
// by the origins on the link; revoking the link ends it.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };

test.describe.configure({ mode: "serial" });

let admin: BrowserContext;
let adminPage: Page;
let token = "";
let linkId = 0;

test.beforeAll(async ({ browser }) => {
  admin = await browser.newContext();
  adminPage = await admin.newPage();
});

test.afterAll(async () => {
  await admin.close();
});

async function signInAsAdmin() {
  await adminPage.goto("/");
  const setup = adminPage.getByRole("heading", { name: "Set up Superhet" });
  const signIn = adminPage.getByRole("heading", { name: "Sign in" });
  const tuning = adminPage.getByRole("region", { name: "Tuning" });
  await expect(setup.or(signIn).or(tuning)).toBeVisible();
  if (await setup.isVisible()) {
    await adminPage.getByLabel("Username").fill(ADMIN.username);
    await adminPage.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await adminPage.getByLabel("Confirm password").fill(ADMIN.password);
    await adminPage.getByLabel("Setup token").fill(SETUP_TOKEN);
    await adminPage.getByRole("button", { name: "Create account" }).click();
  } else if (await signIn.isVisible()) {
    await adminPage.getByLabel("Username").fill(ADMIN.username);
    await adminPage.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await adminPage.getByRole("button", { name: "Sign in" }).click();
  }
  await expect(tuning).toBeVisible();
}

async function api(method: string, url: string, body?: unknown) {
  return adminPage.evaluate(
    async ([m, u, b]) => {
      const res = await fetch(u as string, {
        method: m as string,
        headers: b === undefined ? {} : { "Content-Type": "application/json" },
        body: b === undefined ? undefined : JSON.stringify(b),
      });
      return { status: res.status, body: res.status === 204 ? null : await res.json() };
    },
    [method, url, body] as const,
  );
}

test("an admin makes a share link", async () => {
  await signInAsAdmin();
  const made = await api("POST", "/api/v1/admin/share-links", {
    label: "e2e",
    title: "E2E hilltop",
    description: "Smoke test station",
    location: "Test lab",
    device: "dev0",
    tuning: "follow",
    features: ["waterfall", "audio"],
    max_listeners: 3,
    embed_origins: ["https://blog.example.org"],
  });
  expect(made.status, JSON.stringify(made.body)).toBe(201);
  token = made.body.token as string;
  linkId = made.body.link.id as number;
  expect(token.length).toBeGreaterThan(30);
  // The list never repeats the token.
  const list = await api("GET", "/api/v1/admin/share-links");
  expect(JSON.stringify(list.body)).not.toContain(token);

  await adminPage.goto("/admin/share");
  await expect(adminPage.getByRole("list", { name: "Share links" })).toContainText("e2e");
});

test("a visitor without a login sees the station and the waterfall stream", async ({ browser }) => {
  const visitor = await browser.newContext();
  const page = await visitor.newPage();
  const spectrum = page.waitForEvent("websocket", (ws) =>
    ws.url().includes(`/api/v1/share/${token}/ws/spectrum`),
  );
  const response = await page.goto(`/r/${token}`);
  // The public page itself can never be framed.
  expect(response?.headers()["content-security-policy"]).toContain("frame-ancestors 'none'");
  await expect(page.getByRole("heading", { name: "E2E hilltop" })).toBeVisible();
  await expect(page.getByText("Smoke test station")).toBeVisible();
  await spectrum;
  await expect(page.getByLabel("Waterfall")).toBeVisible();
  await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
  // Nothing was asked of the browser's cookie jar, and the account API stays closed.
  expect(await visitor.cookies()).toHaveLength(0);
  const closed = await page.evaluate(async () => (await fetch("/api/v1/devices")).status);
  expect(closed).toBe(401);
  await visitor.close();
});

test("only the embed page of the link may be framed, and only by its origins", async ({
  request,
}) => {
  const embed = await request.get(`/embed/${token}`);
  expect(embed.headers()["content-security-policy"]).toContain(
    "frame-ancestors https://blog.example.org",
  );
  expect(embed.headers()["x-frame-options"]).toBeUndefined();
  for (const path of ["/embed/not-a-link", `/r/${token}`, "/"]) {
    const other = await request.get(path);
    expect(other.headers()["content-security-policy"], path).toContain("frame-ancestors 'none'");
    expect(other.headers()["x-frame-options"], path).toBe("DENY");
  }
});

test("revoking the link ends it", async ({ browser }) => {
  const revoked = await api("POST", `/api/v1/admin/share-links/${linkId}/revoke`);
  expect(revoked.status).toBe(200);
  const visitor = await browser.newContext();
  const page = await visitor.newPage();
  await page.goto(`/r/${token}`);
  await expect(page.getByText("This link was withdrawn by the station.")).toBeVisible();
  await page.goto("/r/not-a-link");
  await expect(page.getByText("This link is not valid.")).toBeVisible();
  await visitor.close();
});
