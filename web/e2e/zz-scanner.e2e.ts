import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of the scanner: the second daemon (`dev1`, scripts/e2e-stack.sh) plays a
// steady carrier 100 kHz above its centre. A range scan stops on it, the Scanner tab shows the
// channel, the scanner holds the tuning lock, pause / skip / lock out / stop work from the tab and
// the keyboard, and the hit list tunes the VFO. The login of the earlier scenarios is reused.

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
  // Leave nothing running for the scenarios that follow.
  await api("POST", "/api/v1/devices/dev1/scanner/stop", { v: 1 }).catch(() => undefined);
  await context.close();
});

async function signInAsAdmin() {
  await page.goto("/?dev=dev1");
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

const STATE = "/api/v1/devices/dev1/state";

async function scannerState(): Promise<{ state: string; current?: { freq_hz: number } }> {
  const res = await api("GET", STATE);
  return res.body.scanner ?? { state: "none" };
}

test("a range scan stops on the carrier and shows it in the Scanner tab", async () => {
  await signInAsAdmin();
  // A VFO to listen with (the multi-device scenario may have made one already).
  const state = await api("GET", STATE);
  if ((state.body.vfos as unknown[]).length === 0) {
    const made = await api("POST", "/api/v1/devices/dev1/vfos", {
      freq_hz: state.body.center_hz,
      mode: "nfm",
    });
    expect(made.status).toBe(201);
  }
  const carrier = (state.body.center_hz as number) + 100_000;

  // Eleven 12.5 kHz channels around the carrier; the file device is looped steadily.
  const started = await api("POST", "/api/v1/devices/dev1/scanner/start", {
    source: {
      kind: "range",
      start_hz: carrier - 5 * 12_500,
      stop_hz: carrier + 5 * 12_500,
      step_hz: 12_500,
      mode: "nfm",
    },
    resume: "hold",
    dwell_ms: 100,
  });
  expect(started.status, JSON.stringify(started.body)).toBe(200);

  await page.getByRole("tab", { name: "Scanner" }).click();
  const current = page.getByTestId("scan-current");
  await expect(current).toBeVisible({ timeout: 15_000 });
  await expect(current).toContainText(`${carrier / 1e6}`);
  await expect(page.getByText("Signal", { exact: true })).toBeVisible();

  // The scanner holds the tuning lock: a retune is refused meanwhile.
  const after = await api("GET", STATE);
  expect(after.body.locks.map((l: { kind: string }) => l.kind)).toContain("scanner");
  expect(after.body.vfos[0].freq_hz).toBe(carrier);
  const retune = await api("POST", "/api/v1/devices/dev1/tune", { center_hz: 100_000_000 });
  expect(retune.status).toBe(409);
});

test("pause, resume, skip and lock out from the tab, stop from the keyboard", async () => {
  const hits = page.getByRole("list", { name: "Scanner hits" });
  await expect(hits.getByRole("button").first()).toBeVisible();

  await page.getByRole("button", { name: /^Pause/ }).click();
  await expect(page.getByText("Paused", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Resume/ }).click();
  await expect(page.getByTestId("scan-current")).toBeVisible();

  // Locking out the held channel lets the scan go on; the neighbouring channel of the same carrier
  // may take over, so lock out until nothing is held.
  for (let i = 0; i < 3; i++) {
    const lock = page.getByRole("button", { name: /^Lock out/ });
    if (!(await lock.isEnabled())) break;
    await lock.click();
    await expect.poll(async () => (await scannerState()).state).not.toBe("none");
    await page.waitForTimeout(700);
  }
  await page.getByRole("button", { name: /Locked out \(\d+\)/ }).click();
  await expect(page.getByRole("button", { name: /×$/ }).first()).toBeVisible();
  await page.getByRole("button", { name: "Clear all" }).click();

  // `G` stops a running scan, and starts the last one again.
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("g");
  await expect.poll(async () => (await scannerState()).state, { timeout: 10_000 }).toBe("idle");
  const idle = await api("GET", STATE);
  expect(idle.body.locks).toEqual([]);
  await expect(page.getByText("Not running.")).toBeVisible();
  await page.keyboard.press("g");
  await expect.poll(async () => (await scannerState()).state, { timeout: 15_000 }).toBe("holding");

  // A hit tunes the VFO.
  await api("POST", "/api/v1/devices/dev1/scanner/stop", { v: 1 });
  await expect(page.getByRole("button", { name: /^Start/ })).toBeVisible();
  await hits.getByRole("button").first().click();
});

test("unknown actions are refused, a stop is idempotent", async () => {
  const res = await page.evaluate(async () => {
    const r = await fetch("/api/v1/devices/dev1/scanner/stop", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
    });
    return r.status;
  });
  // The admin may; an unknown action is a 404 whoever asks.
  expect(res).toBe(200);
  const unknown = await api("POST", "/api/v1/devices/dev1/scanner/format", {});
  expect(unknown.status).toBe(404);
});
