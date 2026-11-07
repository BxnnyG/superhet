import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of several devices (ADR-0020): the stack from scripts/e2e-stack.sh runs two
// daemons on file devices, `dev0` (100 MHz) and `dev1` ("Second stick", 145.5 MHz). The device
// picker lists both, switching moves the spectrum socket to the other device, and tuning or
// creating a VFO on one leaves the other untouched. The login of the earlier scenarios is reused.

const SETUP_TOKEN = process.env.E2E_SETUP_TOKEN ?? "e2e-setup-token-0123456789";
const ADMIN = { username: "root", password: "admin passphrase for the smoke test" };

test.describe.configure({ mode: "serial" });

let context: BrowserContext;
let page: Page;

test.beforeAll(async ({ browser }) => {
  context = await browser.newContext();
  page = await context.newPage();
});

test.afterAll(async () => {
  await context.close();
});

async function signInAsAdmin() {
  await page.goto("/");
  const setup = page.getByRole("heading", { name: "Set up Superhet" });
  const signIn = page.getByRole("heading", { name: "Sign in" });
  const tuning = page.getByRole("region", { name: "Tuning" });
  await expect(setup.or(signIn).or(tuning)).toBeVisible();
  if (await setup.isVisible()) {
    await page.getByLabel("Username").fill(ADMIN.username);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await page.getByLabel("Confirm password").fill(ADMIN.password);
    await page.getByLabel("Setup token").fill(SETUP_TOKEN);
    await page.getByRole("button", { name: "Create account" }).click();
  } else if (await signIn.isVisible()) {
    await page.getByLabel("Username").fill(ADMIN.username);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await page.getByRole("button", { name: "Sign in" }).click();
  }
  await expect(tuning).toBeVisible();
}

/** JSON call from inside the page, so the session cookie and origin are the browser's. */
async function api(method: string, url: string, body?: unknown) {
  return page.evaluate(
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

const spectrumSocket = (dev: string) =>
  page.waitForEvent(
    "websocket",
    (ws) => ws.url().includes("/ws/spectrum") && ws.url().includes(`dev=${dev}`),
  );

test("both daemons are listed and answer independently", async () => {
  await signInAsAdmin();
  const list = await api("GET", "/api/v1/devices");
  expect(list.status, JSON.stringify(list.body)).toBe(200);
  const devices = list.body.devices as { id: string; online: boolean; label?: string }[];
  expect(devices.map((d) => d.id)).toEqual(["dev0", "dev1"]);
  expect(devices.every((d) => d.online)).toBe(true);
  expect(devices.find((d) => d.id === "dev1")?.label).toBe("Second stick");

  const [s0, s1] = await Promise.all([
    api("GET", "/api/v1/devices/dev0/state"),
    api("GET", "/api/v1/devices/dev1/state"),
  ]);
  expect(s1.body.center_hz).toBe(145_500_000);
  expect(s0.body.center_hz).not.toBe(s1.body.center_hz);
});

test("tuning and VFOs on one device leave the other alone", async () => {
  const before = await api("GET", "/api/v1/devices/dev0/state");
  const vfosBefore = (before.body.vfos as unknown[]).length;

  const tuned = await api("POST", "/api/v1/devices/dev1/tune", { center_hz: 146_000_000 });
  expect(tuned.status).toBe(200);
  const created = await api("POST", "/api/v1/devices/dev1/vfos", {
    freq_hz: 146_000_000,
    mode: "nfm",
  });
  expect(created.status).toBe(201);

  const after0 = await api("GET", "/api/v1/devices/dev0/state");
  expect(after0.body.center_hz).toBe(before.body.center_hz);
  expect((after0.body.vfos as unknown[]).length).toBe(vfosBefore);
  const after1 = await api("GET", "/api/v1/devices/dev1/state");
  expect(after1.body.center_hz).toBe(146_000_000);
  expect((after1.body.vfos as unknown[]).length).toBeGreaterThan(vfosBefore);
});

test("the picker switches the whole receiver to the other device and remembers it", async () => {
  await page.goto("/");
  await expect(page.getByRole("region", { name: "Tuning" })).toBeVisible();
  await page.getByRole("button", { name: /dev0/ }).click();
  const devices = page.getByRole("list", { name: "Devices" });
  await expect(devices.getByRole("button")).toHaveCount(2);
  await expect(devices.getByText("Second stick")).toBeVisible();

  const socket = spectrumSocket("dev1");
  await devices.getByRole("button", { name: /Second stick/ }).click();
  await socket;
  // The pill now carries the label of the new device.
  await expect(page.getByRole("button", { name: /Second stick/ }).first()).toBeVisible();
  // Its VFO from the previous test is listed on the new device's tab row.
  await expect(page.getByRole("group", { name: "VFOs" })).toContainText("146");

  // The choice survives a reload; ?dev= overrides it for one visit.
  const again = spectrumSocket("dev1");
  await page.reload();
  await again;
  const back = spectrumSocket("dev0");
  await page.goto("/?dev=dev0");
  await back;
  await expect(page.getByRole("button", { name: /dev0/ }).first()).toBeVisible();
});
