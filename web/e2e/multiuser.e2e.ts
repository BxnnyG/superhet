import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";

// Browser smoke test of the multi-user flow (ROADMAP P1.5 criterion 8): first-run setup, an
// invitation accepted as listener, role-dependent controls, guest access, kicking a guest, the
// audit log and its CSV export. One ordered scenario on one fresh stack (scripts/e2e-stack.sh);
// each person gets their own browser context so cookies never mix.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };
const LISTENER = { username: "lena", password: "listener passphrase for the smoke test" };

test.describe.configure({ mode: "serial" });

let browser: Browser;
let admin: { context: BrowserContext; page: Page };
let inviteUrl = "";

async function person(): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext();
  return { context, page: await context.newPage() };
}

test.beforeAll(async ({ browser: b }) => {
  browser = b;
  admin = await person();
});

test.afterAll(async () => {
  await admin.context.close();
});

/** The receiver has loaded: the tuning strip is on screen. */
async function expectReceiver(page: Page) {
  await expect(page.getByRole("region", { name: "Tuning" })).toBeVisible();
}

/** Open the Source tab of the inspector (tuning lock and profiles live there). */
async function openSource(page: Page) {
  await page.getByRole("tab", { name: "Source" }).click();
  await expect(page.getByRole("tabpanel", { name: "Source" })).toBeVisible();
  return page.getByRole("tabpanel", { name: "Source" });
}

test("the first visitor sets up the administrator", async () => {
  const { page } = admin;
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Set up Superhet" })).toBeVisible();
  await page.getByLabel("Username").fill(ADMIN.username);
  await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
  await page.getByLabel("Confirm password").fill(ADMIN.password);
  await page.getByLabel("Setup token").fill(SETUP_TOKEN);
  await page.getByRole("button", { name: "Create account" }).click();
  await expectReceiver(page);
  // Operator controls are live for the admin.
  const source = await openSource(page);
  await expect(source.getByRole("button", { name: "Lock tuning" })).toBeEnabled();
  await expect(source.getByText("Profiles", { exact: true })).toBeVisible();
  // The account menu leads to the admin console.
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("button", { name: "Admin console" })).toBeVisible();
});

test("the inspector tabs fit at desktop width without scrolling", async () => {
  const { page } = admin;
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");
  await expectReceiver(page);
  const list = page.getByRole("tablist", { name: "Inspector" });
  await expect(list).toBeVisible();
  expect(await list.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
  expect(await list.evaluate((el) => el.getAttribute("data-mode"))).not.toBe("full");
  // Collapsed tabs keep their name for assistive technology and stay clickable.
  for (const name of ["Receiver", "Bookmarks", "Channels", "Decoders", "Display", "Source"]) {
    await expect(page.getByRole("tab", { name })).toBeVisible();
  }
  await page.getByRole("tab", { name: "Display" }).click();
  await expect(page.getByRole("tab", { name: "Display" })).toHaveAttribute("aria-selected", "true");
});

test("the admin creates an invitation for a listener", async () => {
  const { page } = admin;
  await page.goto("/admin/users");
  await page.getByRole("button", { name: "Invite", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Invite someone" });
  await dialog.getByLabel("Role").selectOption("listener");
  await dialog.getByLabel("Note (optional)").fill("smoke test");
  await dialog.getByRole("button", { name: /^Create/ }).click();
  const link = page.getByRole("dialog", { name: "Invitation link" });
  inviteUrl = await link.getByRole("textbox").inputValue();
  expect(inviteUrl).toContain("/invite/");
  await link.getByRole("button", { name: "Done" }).click();
});

test("the invitation creates a listener who sees no operator controls", async () => {
  const listener = await person();
  try {
    const { page } = listener;
    await page.goto(new URL(inviteUrl).pathname);
    await expect(page.getByRole("heading", { name: "Join Superhet" })).toBeVisible();
    await page.getByLabel("Username").fill(LISTENER.username);
    await page.getByLabel("Password", { exact: true }).fill(LISTENER.password);
    await page.getByLabel("Confirm password").fill(LISTENER.password);
    await page.getByRole("button", { name: "Create account" }).click();
    await expectReceiver(page);

    // The tuning lock exists but is disabled; the Source tab has no Profiles section.
    const source = await openSource(page);
    await expect(source.getByRole("button", { name: "Lock tuning" })).toBeDisabled();
    await expect(source.getByText("Profiles", { exact: true })).toHaveCount(0);

    // No admin navigation, and the server agrees.
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(page.getByRole("button", { name: "My sessions" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Admin console" })).toHaveCount(0);
    expect((await page.request.get("/api/v1/admin/users")).status()).toBe(403);
    expect((await page.request.get("/api/v1/profiles")).status()).toBe(403);
    await page.goto("/admin");
    await expect(page.getByRole("navigation", { name: "Admin sections" })).toHaveCount(0);
  } finally {
    await listener.context.close();
  }
});

test("the admin enables guest access", async () => {
  const { page } = admin;
  await page.goto("/admin/guests");
  const toggle = page.getByRole("switch", { name: "Allow guest access" });
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(toggle).toBeChecked();
  await expect(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
});

test("a guest enters, is listed, and is kicked by the admin", async () => {
  const guest = await person();
  try {
    await guest.page.goto("/");
    await guest.page.getByRole("button", { name: "Continue as guest" }).click();
    await guest.page.getByRole("button", { name: "Accept and continue as guest" }).click();
    await expectReceiver(guest.page);
    const source = await openSource(guest.page);
    await expect(source.getByRole("button", { name: "Lock tuning" })).toBeDisabled();

    const { page } = admin;
    await page.goto("/admin/sessions");
    const row = page.getByRole("listitem").filter({ hasText: /Guest [0-9a-f]{6}/ });
    await expect(row).toHaveCount(1);
    // The test browsers share the admin's address, so banning it is refused by the safeguard.
    await row.getByRole("button", { name: "Kick and ban" }).click();
    const banDialog = page.getByRole("dialog", { name: /Kick and ban/ });
    await banDialog.getByRole("button", { name: "Kick and ban" }).click();
    await expect(banDialog.getByText(/block your own address/)).toBeVisible();
    await banDialog.getByRole("button", { name: "Cancel" }).click();
    await expect(row).toHaveCount(1);

    await row.getByRole("button", { name: "Kick", exact: true }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Kick", exact: true }).click();
    await expect(row).toHaveCount(0);

    // The guest's page falls back to the sign-in screen.
    await expect(guest.page.getByRole("heading", { name: "Sign in" })).toBeVisible({
      timeout: 15_000,
    });
  } finally {
    await guest.context.close();
  }
});

test("browsers report their device and public IP once the admin asks for it", async () => {
  const { page } = admin;
  await page.goto("/admin/client-info");
  await page.getByLabel("Ask the browsers of").selectOption("all");
  await page.getByRole("switch", { name: "Ask clients for their public IP" }).click();
  await expect(page.getByText(/the third party sees their IP address/)).toBeVisible();
  await page.getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByText("All changes saved")).toBeVisible();

  const guest = await person();
  // The lookup service is stubbed; the request still has to pass the page's CSP.
  await guest.page.route("https://api.ipify.org/*", (route) =>
    route.fulfill({ json: { ip: "198.51.100.23" } }),
  );
  try {
    await guest.page.goto("/");
    await guest.page.getByRole("button", { name: "Continue as guest" }).click();
    const notice = guest.page.getByRole("region", { name: "Device details" });
    await expect(notice).toContainText("public IP address");
    await expect(notice).toContainText("api.ipify.org");
    await guest.page.getByRole("button", { name: "Accept and continue as guest" }).click();
    await expectReceiver(guest.page);

    await page.goto("/admin/sessions");
    const row = page.getByRole("listitem").filter({ hasText: /Guest [0-9a-f]{6}/ });
    await expect(row.getByText("client-reported · unverified").first()).toBeVisible({
      timeout: 15_000,
    });
    // Guests are truncated like their own address; the claim is not the address the server sees.
    await expect(row).toContainText("198.51.100.0");
    await row
      .getByText(/public IP/)
      .first()
      .click();
    await expect(row.getByText("Public IP (client-reported)")).toBeVisible();
  } finally {
    await guest.context.close();
  }

  await page.goto("/admin/audit");
  await expect(
    page
      .getByRole("table", { name: "Audit events" })
      .getByText("session.client_report", { exact: true })
      .first(),
  ).toBeVisible();
});

test("the admin bans and lifts a network", async () => {
  const { page } = admin;
  await page.goto("/admin/bans");
  await expect(page.getByText("No bans")).toBeVisible();
  await page.getByRole("button", { name: "Ban an address" }).click();
  const dialog = page.getByRole("dialog", { name: "Ban an address" });
  await dialog.getByLabel("Address or network").fill("203.0.113.77/24");
  await dialog.getByLabel("Reason (optional)").fill("smoke test");
  await dialog.getByRole("button", { name: "Ban", exact: true }).click();
  const row = page.getByRole("listitem").filter({ hasText: "203.0.113.0/24" });
  await expect(row).toContainText("smoke test");

  // Banning the address of the admin's own browser is refused.
  await page.getByRole("button", { name: "Ban an address" }).click();
  await dialog.getByLabel("Address or network").fill("127.0.0.1");
  await dialog.getByRole("button", { name: "Ban", exact: true }).click();
  await expect(dialog.getByText(/block your own address/)).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();

  await row.getByRole("button", { name: "Lift" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Lift ban" }).click();
  await expect(page.getByText("No bans")).toBeVisible();
});

test("the audit log shows the events and exports them as CSV", async () => {
  const { page } = admin;
  await page.goto("/admin/audit");
  const table = page.getByRole("table", { name: "Audit events" });
  for (const action of [
    "auth.setup",
    "invite.create",
    "invite.accept",
    "guest.policy_change",
    "guest.session_start",
    "session.kick",
    "ban.create",
    "ban.lift",
  ]) {
    await expect(table.getByText(action, { exact: true }).first()).toBeVisible();
  }

  const download = page.waitForEvent("download");
  await page.getByRole("link", { name: "Export CSV" }).click();
  const file = await download;
  expect(file.suggestedFilename()).toMatch(/\.csv$/);
  const path = await file.path();
  const csv = await readFile(path, "utf8");
  for (const action of ["auth.setup", "invite.accept", "session.kick"]) {
    expect(csv).toContain(action);
  }
  expect(csv).toContain(LISTENER.username);
});
