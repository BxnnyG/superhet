import { expect, test, type BrowserContext, type Page } from "@playwright/test";

// Browser smoke test of recording and instant replay on the second file device (`dev1`, steady
// carrier): start an audio recording from the Receiver tab, stop it, find it in the Recordings tab,
// play it (the player loads the Ogg file), rename, keep, download and delete it, then replay the
// buffer, save it and record IQ as a SigMF pair. The login of the earlier scenarios is reused.

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

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Reply = { status: number; body: any };

async function api(method: string, url: string, body?: unknown): Promise<Reply> {
  return page.evaluate(
    async ([m, u, b]) => {
      const res = await fetch(u as string, {
        method: m as string,
        headers: b === undefined ? {} : { "Content-Type": "application/json" },
        body: b === undefined ? undefined : JSON.stringify(b),
      });
      const text = await res.text();
      let parsed: unknown;
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = null;
      }
      return { status: res.status, body: parsed };
    },
    [method, url, body] as const,
  );
}

test("record audio from the Receiver tab, list it, play it", async () => {
  await page.goto("/?dev=dev1");
  const signIn = page.getByRole("heading", { name: "Sign in" });
  const tuning = page.getByRole("region", { name: "Tuning" });
  await expect(signIn.or(tuning)).toBeVisible();
  if (await signIn.isVisible()) {
    await page.getByLabel("Username").fill(ADMIN.username);
    await page.getByLabel("Password", { exact: true }).fill(ADMIN.password);
    await page.getByRole("button", { name: "Sign in" }).click();
  }
  await expect(tuning).toBeVisible();

  const state = await api("GET", "/api/v1/devices/dev1/state");
  if ((state.body.vfos as unknown[]).length === 0) {
    const made = await api("POST", "/api/v1/devices/dev1/vfos", {
      freq_hz: (state.body.center_hz as number) + 100_000,
      mode: "nfm",
    });
    expect(made.status).toBe(201);
  }

  await page.getByRole("tab", { name: "Receiver" }).click();
  await page.getByRole("heading", { name: "Record and replay" }).scrollIntoViewIfNeeded();
  await page.getByRole("button", { name: "Record", exact: true }).click();
  const stop = page.getByRole("button", { name: /^Stop recording/ });
  await expect(stop).toBeVisible();
  // Let some audio arrive (50 packets a second).
  await page.waitForTimeout(3000);
  await stop.click();
  await expect(page.getByRole("button", { name: "Record", exact: true })).toBeVisible();

  await page.getByRole("tab", { name: "Recordings" }).click();
  const row = page.getByTestId("recording-row").first();
  await expect(row).toBeVisible();
  await row.getByRole("button", { name: "Play" }).click();
  const player = row.locator("audio");
  await expect(player).toBeVisible();
  await expect
    .poll(async () => player.evaluate((a: HTMLAudioElement) => a.readyState), { timeout: 10_000 })
    .toBeGreaterThan(0);

  // The file is a valid Ogg stream and supports Range requests.
  const list = await api("GET", "/api/v1/recordings?dev=dev1&kind=audio");
  expect(list.status).toBe(200);
  const rec = list.body.recordings[0];
  expect(rec.state).toBe("done");
  expect(rec.duration_ms).toBeGreaterThan(1000);
  const head = await page.evaluate(async (id) => {
    const res = await fetch(`/api/v1/recordings/${id}/file`, { headers: { Range: "bytes=0-3" } });
    return { status: res.status, magic: new TextDecoder().decode(await res.arrayBuffer()) };
  }, rec.id as number);
  expect(head).toEqual({ status: 206, magic: "OggS" });
});

test("rename, keep, download and delete", async () => {
  const row = page.getByTestId("recording-row").first();
  await row.getByRole("button", { name: "Rename" }).click();
  const field = row.getByLabel("Title");
  await field.fill("Smoke test call");
  await field.press("Enter");
  await expect(row.getByText("Smoke test call")).toBeVisible();
  await row.getByRole("button", { name: /^Keep/ }).click();
  await expect(row.getByRole("button", { name: /Allow automatic cleanup/ })).toBeVisible();

  const downloadPromise = page.waitForEvent("download");
  await row.getByRole("link", { name: "Download" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("Smoke_test_call.ogg");

  await row.getByRole("button", { name: "Delete" }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();
  await expect(page.getByText("Smoke test call")).toHaveCount(0);
});

test("instant replay plays and saves the buffered audio", async () => {
  await page.getByRole("tab", { name: "Receiver" }).click();
  await expect
    .poll(async () => (await api("GET", "/api/v1/devices/dev1/vfos/1/replay?seconds=10")).status, {
      timeout: 20_000,
    })
    .toBe(200);
  await page.getByRole("button", { name: "Replay last 10 seconds" }).click();
  const saved = await api("POST", "/api/v1/devices/dev1/vfos/1/replay/save", { seconds: 10 });
  expect(saved.status, JSON.stringify(saved.body)).toBe(201);
  expect(saved.body.source).toBe("replay");
  expect(saved.body.duration_ms).toBeGreaterThan(500);
  const gone = await api("DELETE", `/api/v1/recordings/${saved.body.id}`);
  expect(gone.status).toBe(200);
});

test("IQ of a VFO is a SigMF pair", async () => {
  const made = await api("POST", "/api/v1/devices/dev1/recordings", {
    kind: "iq",
    source: "vfo",
    vfo: 1,
    rate: 48_000,
    max_seconds: 3,
  });
  expect(made.status, JSON.stringify(made.body)).toBe(201);
  await expect
    .poll(async () => (await api("GET", `/api/v1/recordings/${made.body.id}`)).body.state, {
      timeout: 15_000,
    })
    .toBe("done");
  const meta = await api("GET", `/api/v1/recordings/${made.body.id}/file?part=meta`);
  expect(meta.status).toBe(200);
  expect(meta.body.global["core:datatype"]).toBe("ci16_le");
  expect(meta.body.global["core:sample_rate"]).toBe(48_000);
  expect(meta.body.captures[0]["core:sample_start"]).toBe(0);
  await api("DELETE", `/api/v1/recordings/${made.body.id}`);
});
